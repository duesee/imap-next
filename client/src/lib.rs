#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

use std::{error::Error, fmt::Debug, num::NonZeroU32, sync::Arc};

use anyhow::Result;
use imap_codec::imap_types::{
    command::{Command, CommandBody},
    core::{NonEmptyVec, QuotedChar, Tag},
    fetch::{MacroOrMessageDataItemNames, MessageDataItem},
    flag::{Flag, FlagNameAttribute, FlagPerm},
    mailbox::{ListMailbox, Mailbox},
    response::{Capability, Code, Data, Greeting, Status},
    sequence::SequenceSet,
};
use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::AnyStream,
};
use tokio::net::TcpStream;
use tokio_rustls::{
    rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName},
    TlsConnector,
};
use tracing::{debug, instrument, warn, Span};

#[derive(Debug)]
pub struct Client {
    flow: ClientFlow,
    tag_generator: TagGenerator,
}

/*
struct MailboxHigh {
    Inbox(String),
    Other(String),
}
*/

impl Client {
    #[instrument]
    pub async fn receive_greeting(
        name: &str,
        port: u16,
    ) -> Result<(Greeting<'static>, Self), Box<dyn Error>> {
        let stream = {
            let stream = TcpStream::connect((name, port)).await?;

            let mut root_cert_store = RootCertStore::empty();
            #[allow(deprecated)]
            root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
                OwnedTrustAnchor::from_subject_spki_name_constraints(
                    ta.subject,
                    ta.spki,
                    ta.name_constraints,
                )
            }));
            let config = ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_cert_store)
                .with_no_client_auth();
            let connector = TlsConnector::from(Arc::new(config));
            let dnsname = ServerName::try_from(name).unwrap();

            connector.connect(dnsname, stream).await?
        };

        let (flow, greeting) =
            ClientFlow::receive_greeting(AnyStream::new(stream), ClientFlowOptions::default())
                .await?;

        Ok((
            greeting,
            Self {
                flow,
                tag_generator: TagGenerator { tag_count: 1 },
            },
        ))
    }

    #[instrument(skip_all, fields(username))]
    pub async fn login<U, P>(
        &mut self,
        username: U,
        password: P,
    ) -> Result<Option<NonEmptyVec<Capability<'static>>>, Box<dyn Error>>
    where
        U: AsRef<[u8]>,
        P: AsRef<[u8]>,
    {
        Span::current().record(
            "username",
            std::str::from_utf8(username.as_ref()).unwrap_or("<non-UTF8 username>"),
        );

        // RFC 3501:
        //
        // ```text
        // Arguments: user name, password
        //
        // Responses: no specific responses for this command
        //
        // Result: OK - login completed, now in authenticated state
        //         NO - login failure: user name or password rejected
        //         BAD - command unknown or arguments invalid
        // ```
        let cmd = {
            // TODO: Handle non-sync literals?
            let tag = self.tag_generator.generate();
            let body = CommandBody::login(username.as_ref(), password.as_ref())?;

            Command { tag, body }
        };

        let _handle = self.flow.enqueue_command(cmd.clone());

        let mut _capabilities: Option<NonEmptyVec<Capability<'static>>> = None;

        loop {
            match self.flow.progress().await? {
                // TODO: Refactor: We only ever need the handle, do we?
                ClientFlowEvent::CommandSent { handle, .. } => {
                    if handle != _handle {
                        todo!();
                    }
                }
                ClientFlowEvent::DataReceived { data } => match data {
                    // Non-standard but observed in Zoho Mail (Oct. 2023)
                    Data::Capability(capabilities) => {
                        if _capabilities.is_some() {
                            warn!(?capabilities, "received duplicate capabilities");
                        }

                        _capabilities = Some(capabilities);
                    }
                    _ => warn!(?data, "unexpected data"),
                },
                ClientFlowEvent::StatusReceived { status } => {
                    if status.tag() == Some(&cmd.tag) {
                        match status {
                            Status::Ok { code, .. } => {
                                if let Some(Code::Capability(capabilities)) = code {
                                    if _capabilities.is_some() {
                                        warn!(duplicate=?capabilities, "received duplicate capabilities");
                                    }

                                    _capabilities = Some(capabilities);
                                }

                                return Ok(_capabilities);
                            }
                            _ => return Err("login failed".into()),
                        }
                    } else if matches!(status, Status::Bye { .. }) {
                        return Err("server closed the connection".into());
                    } else {
                        warn!(?status, "unexpected data");
                    }
                }
            }
        }
    }

    #[instrument(skip_all)]
    pub async fn list<'a>(
        &mut self,
        reference: &str,
        mailbox_wildcard: &str,
    ) -> Result<
        Vec<(
            Vec<FlagNameAttribute<'static>>,
            Option<QuotedChar>,
            Mailbox<'static>,
        )>,
        Box<dyn Error>,
    > {
        let cmd = {
            let reference = Mailbox::try_from(reference)?;
            let mailbox_wildcard = ListMailbox::try_from(mailbox_wildcard)?;

            let tag = self.tag_generator.generate();
            let body = CommandBody::List {
                reference,
                mailbox_wildcard,
            };

            Command { tag, body }
        };

        let _handle = self.flow.enqueue_command(cmd.clone());

        let mut lists = vec![];

        loop {
            match self.flow.progress().await? {
                // TODO: Refactor: We only ever need the handle, do we?
                ClientFlowEvent::CommandSent { handle, .. } => {
                    if handle != _handle {
                        todo!();
                    }
                }
                ClientFlowEvent::DataReceived { data } => match data {
                    Data::List {
                        items,
                        delimiter,
                        mailbox,
                    } => {
                        lists.push((items, delimiter, mailbox));
                    }
                    _ => warn!(?data, "unexpected data"),
                },
                ClientFlowEvent::StatusReceived { status } => {
                    if status.tag() == Some(&cmd.tag) {
                        if matches!(status, Status::Ok { .. }) {
                            return Ok(lists);
                        } else {
                            return Err("list failed".into());
                        }
                    } else if matches!(status, Status::Bye { .. }) {
                        return Err("server closed the connection".into());
                    } else {
                        warn!(?status, "unexpected status");
                    }
                }
            }
        }
    }

    #[instrument(skip_all)]
    pub async fn select(&mut self, mailbox: Mailbox<'static>) -> Result<Session, Box<dyn Error>> {
        let cmd = {
            let tag = self.tag_generator.generate();
            let body = CommandBody::Select { mailbox };

            Command { tag, body }
        };

        self.flow.enqueue_command(cmd.clone());

        self.handle_select_or_examine(cmd).await
    }

    #[instrument(skip_all)]
    pub async fn examine(&mut self, mailbox: Mailbox<'static>) -> Result<Session, Box<dyn Error>> {
        let cmd = {
            let tag = self.tag_generator.generate();
            let body = CommandBody::Examine { mailbox };

            Command { tag, body }
        };

        self.flow.enqueue_command(cmd.clone());

        self.handle_select_or_examine(cmd).await
    }

    async fn handle_select_or_examine(
        &mut self,
        cmd: Command<'static>,
    ) -> Result<Session, Box<dyn Error>> {
        let mut session_data = SessionData::default();

        loop {
            match self.flow.progress().await? {
                ClientFlowEvent::CommandSent { .. } => {
                    // ...
                }
                ClientFlowEvent::DataReceived { data } => match data {
                    Data::Flags(flags) => {
                        session_data.flags = Some(flags);
                    }
                    Data::Exists(number) => {
                        session_data.exists = Some(number);
                    }
                    Data::Recent(recent) => {
                        session_data.recent = Some(recent);
                    }
                    _ => {
                        warn!(?data, "unknown data");
                    }
                },
                ClientFlowEvent::StatusReceived { status } => match status {
                    Status::Ok {
                        tag: None,
                        code: Some(code),
                        ..
                    } => match code {
                        Code::Unseen(unseen) => {
                            session_data.unseen = Some(unseen);
                        }
                        Code::PermanentFlags(flags) => {
                            session_data.permanent_flags = Some(flags);
                        }
                        Code::UidNext(uid_next) => {
                            session_data.uid_next = Some(uid_next);
                        }
                        Code::UidValidity(uid_validity) => {
                            session_data.uid_validity = Some(uid_validity);
                        }
                        _ => {
                            warn!(?code, "unexpected code");
                        }
                    },
                    Status::Ok { tag: Some(tag), .. } if tag == cmd.tag => {
                        return Ok(Session {
                            client: self,
                            session_data,
                        });
                    }
                    Status::No { tag: Some(tag), .. } if tag == cmd.tag => {
                        return Err("no".into());
                    }
                    Status::Bad { tag: Some(tag), .. } if tag == cmd.tag => {
                        return Err("bad".into());
                    }
                    Status::Bye { .. } => {
                        return Err("server closed the connection".into());
                    }
                    _ => {
                        warn!(?status, "unexpected status");
                    }
                },
            }
        }
    }
}

#[derive(Debug)]
pub struct Session<'a> {
    client: &'a mut Client,
    #[allow(unused)]
    pub session_data: SessionData,
}

impl<'s> Session<'s> {
    #[instrument(skip_all)]
    pub async fn fetch<S, I>(
        &mut self,
        sequence_set: S,
        macro_or_data_items: I,
        uid: bool,
    ) -> Result<Vec<(NonZeroU32, NonEmptyVec<MessageDataItem<'static>>)>, Box<dyn Error>>
    where
        S: TryInto<SequenceSet> + Debug,
        I: Into<MacroOrMessageDataItemNames<'static>> + Debug,
        <S as TryInto<SequenceSet>>::Error: Debug + Error + 'static,
    {
        let cmd = {
            let tag = self.client.tag_generator.generate();
            // TODO: Handling of the error is a bit painful here as we require `: Debug`
            let body = CommandBody::fetch(sequence_set, macro_or_data_items, uid)?;

            Command { tag, body }
        };

        self.client.flow.enqueue_command(cmd.clone());

        let mut fetch = vec![];

        loop {
            match self.client.flow.progress().await? {
                ClientFlowEvent::CommandSent { tag, handle } => {
                    debug!(?tag, ?handle, "command sent");
                }
                ClientFlowEvent::DataReceived { data } => match data {
                    Data::Fetch { seq, items } => {
                        fetch.push((seq, items));
                        debug!(?seq, "received fetch");
                    }
                    _ => {
                        warn!(?data, "unexpected data");
                    }
                },
                ClientFlowEvent::StatusReceived { status } => match status {
                    Status::Ok { tag: Some(tag), .. } if tag == cmd.tag => {
                        return Ok(fetch);
                    }
                    Status::No { tag: Some(tag), .. } if tag == cmd.tag => {
                        return Err("no".into());
                    }
                    Status::Bad { tag: Some(tag), .. } if tag == cmd.tag => {
                        return Err("bad".into());
                    }
                    Status::Bye { .. } => {
                        return Err("server closed the connection".into());
                    }
                    _ => {
                        warn!(?status, "unexpected status");
                    }
                },
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionData {
    pub flags: Option<Vec<Flag<'static>>>,
    pub exists: Option<u32>,
    pub recent: Option<u32>,
    pub unseen: Option<NonZeroU32>,
    pub permanent_flags: Option<Vec<FlagPerm<'static>>>,
    pub uid_next: Option<NonZeroU32>,
    pub uid_validity: Option<NonZeroU32>,
}

#[derive(Debug)]
struct TagGenerator {
    tag_count: usize,
}

impl TagGenerator {
    fn generate(&mut self) -> Tag<'static> {
        let next = format!("T{}", self.tag_count);
        self.tag_count = self.tag_count.wrapping_add(1);

        Tag::unvalidated(next)
    }
}
