use std::{cmp::Ordering, collections::HashMap, num::NonZeroU32, sync::Arc, time::Duration};

pub use imap_flow;
use imap_flow::{
    client::{ClientFlow, ClientFlowError, ClientFlowEvent, ClientFlowOptions},
    imap_codec::imap_types::{
        auth::AuthMechanism,
        bounded_static::{IntoBoundedStatic, ToBoundedStatic},
        command::{Command, CommandBody},
        core::{Charset, IString, Literal, LiteralMode, NString, QuotedChar, Tag, Vec1},
        error::ValidationError,
        extensions::{
            binary::{Literal8, LiteralOrLiteral8},
            sort::{SortCriterion, SortKey},
            thread::{Thread, ThreadingAlgorithm},
        },
        fetch::{MacroOrMessageDataItemNames, MessageDataItem, MessageDataItemName},
        flag::{Flag, FlagNameAttribute, StoreType},
        mailbox::{ListMailbox, Mailbox},
        response::{Capability, Code, Status, Tagged},
        search::SearchKey,
        sequence::SequenceSet,
    },
    stream::{Stream, StreamError},
};
use once_cell::sync::Lazy;
pub use tasks;
use tasks::{
    resolver::Resolver,
    tasks::{
        append::{AppendTask, PostAppendCheckTask, PostAppendNoOpTask},
        appenduid::AppendUidTask,
        authenticate::AuthenticateTask,
        capability::CapabilityTask,
        check::CheckTask,
        copy::CopyTask,
        create::CreateTask,
        delete::DeleteTask,
        expunge::ExpungeTask,
        fetch::{FetchFirstTask, FetchTask},
        id::IdTask,
        list::ListTask,
        noop::NoOpTask,
        r#move::MoveTask,
        search::SearchTask,
        select::SelectDataUnvalidated,
        select::SelectTask,
        sort::SortTask,
        starttls::StartTlsTask,
        store::{SilentStoreTask, StoreTask},
        thread::ThreadTask,
        TaskError,
    },
    SchedulerError, SchedulerEvent,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConfig, RootCertStore},
    TlsConnector, TlsStream,
};
use tracing::{debug, trace, warn};

static ROOT_CERT_STORE: Lazy<RootCertStore> = Lazy::new(|| {
    let mut root_store = RootCertStore::empty();

    for cert in rustls_native_certs::load_native_certs().unwrap() {
        root_store.add(cert).unwrap();
    }

    root_store
});

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot connect to server using TCP")]
    ConnectTcp(#[source] tokio::io::Error),
    #[error("cannot connect to server using TLS")]
    ConnectTls(#[source] tokio::io::Error),

    #[error("stream error")]
    Stream(#[from] StreamError<SchedulerError>),
    #[error("validation error")]
    Validation(#[from] ValidationError),

    #[error("cannot receive greeting from server")]
    ReceiveGreeting(#[source] StreamError<SchedulerError>),

    #[error("cannot resolve {1} task")]
    ResolveTask(#[source] TaskError, &'static str),
}

impl ClientError {
    pub fn resolve(task: &'static str) -> impl FnOnce(TaskError) -> Self {
        move |err: TaskError| Self::ResolveTask(err, task)
    }
}

pub struct Client {
    host: String,
    stream: Stream,
    resolver: Resolver,
    capabilities: Vec1<Capability<'static>>,
    idle_timeout: Duration,
}

impl Client {
    async fn tcp(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let host = host.to_string();

        let tcp_stream = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(ClientError::ConnectTcp)?;

        let stream = Stream::insecure(tcp_stream);

        let mut flow_opts = ClientFlowOptions::default();
        flow_opts.crlf_relaxed = true;

        let flow = ClientFlow::new(flow_opts);
        let resolver = Resolver::new(flow);

        Ok(Self {
            host,
            stream,
            resolver,
            capabilities: Vec1::from(Capability::Imap4Rev1),
            idle_timeout: Duration::from_secs(5 * 60), // 5 min
        })
    }

    pub async fn insecure(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let mut client = Self::tcp(host, port).await?;

        if !client.receive_greeting().await? {
            client.refresh_capabilities().await?;
        }

        Ok(client)
    }

    pub async fn starttls(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let tcp_client = Self::tcp(host, port).await?;
        Self::upgrade_tls(tcp_client, true).await
    }

    pub async fn tls(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let tcp_client = Self::tcp(host, port).await?;
        Self::upgrade_tls(tcp_client, false).await
    }

    pub fn set_idle_timeout(&mut self, timeout: Duration) {
        self.idle_timeout = timeout;
    }

    pub fn set_some_idle_timeout(&mut self, timeout: Option<Duration>) {
        if let Some(timeout) = timeout {
            self.set_idle_timeout(timeout)
        }
    }

    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.set_idle_timeout(timeout);
        self
    }

    pub fn with_some_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.set_some_idle_timeout(timeout);
        self
    }

    async fn upgrade_tls(mut self, starttls: bool) -> Result<Self, ClientError> {
        if starttls {
            self.receive_greeting().await?;
            let _ = self
                .stream
                .progress(self.resolver.resolve(StartTlsTask::new()))
                .await;
        }

        let mut config = ClientConfig::builder()
            .with_root_certificates(ROOT_CERT_STORE.clone())
            .with_no_client_auth();

        // See <https://www.iana.org/assignments/tls-extensiontype-values/tls-extensiontype-values.xhtml#alpn-protocol-ids>
        config.alpn_protocols = vec![b"imap".to_vec()];

        let connector = TlsConnector::from(Arc::new(config));
        let dnsname = ServerName::try_from(self.host.clone()).unwrap();

        let tls_stream = connector
            .connect(dnsname, self.stream.into_inner())
            .await
            .map_err(ClientError::ConnectTls)?;

        self.stream = Stream::tls(TlsStream::Client(tls_stream));

        if starttls || !self.receive_greeting().await? {
            self.refresh_capabilities().await?;
        }

        Ok(self)
    }

    async fn receive_greeting(&mut self) -> Result<bool, ClientError> {
        let evt = self
            .stream
            .progress(&mut self.resolver)
            .await
            .map_err(ClientError::ReceiveGreeting)?;

        if let SchedulerEvent::GreetingReceived(greeting) = evt {
            if let Some(Code::Capability(capabilities)) = greeting.code {
                self.capabilities = capabilities;
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn supported_auth_mechanisms(
        &self,
    ) -> impl IntoIterator<Item = AuthMechanism<'static>> + '_ {
        self.capabilities().iter().filter_map(|capability| {
            if let Capability::Auth(mechanism) = capability {
                Some(mechanism.to_static())
            } else {
                None
            }
        })
    }

    pub fn supports_auth_mechanism(&self, mechanism: AuthMechanism<'static>) -> bool {
        self.capabilities.as_ref().iter().any(|capability| {
            if let Capability::Auth(m) = capability {
                *m == mechanism
            } else {
                false
            }
        })
    }

    pub fn capabilities(&self) -> &[Capability<'static>] {
        self.capabilities.as_ref()
    }

    pub async fn refresh_capabilities(&mut self) -> Result<&[Capability<'static>], ClientError> {
        self.capabilities = self
            .stream
            .progress(self.resolver.resolve(CapabilityTask::new()))
            .await?
            .map_err(ClientError::resolve("capability"))?;

        Ok(self.capabilities.as_ref())
    }

    pub fn ext_sasl_ir_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::SaslIr))
    }

    pub fn ext_id_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::Id))
    }

    pub fn ext_uidplus_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::UidPlus))
    }

    pub fn ext_sort_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::Sort(_)))
    }

    pub fn ext_thread_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::Thread(_)))
    }

    pub fn ext_idle_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::Idle))
    }

    pub fn ext_binary_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::Binary))
    }

    pub fn ext_move_supported(&self) -> bool {
        self.capabilities()
            .iter()
            .any(|c| matches!(c, Capability::Move))
    }

    async fn authenticate(
        &mut self,
        task: AuthenticateTask,
        id_params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        let capabilities = self
            .stream
            .progress(self.resolver.resolve(task))
            .await?
            .map_err(ClientError::resolve("authenticate"))?;

        match capabilities {
            Some(capabilities) => {
                self.capabilities = capabilities;
            }
            None => {
                self.refresh_capabilities().await?;
            }
        };

        self.id(id_params).await
    }

    pub async fn authenticate_plain(
        &mut self,
        login: &str,
        passwd: &str,
        id_params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        let ir = self.ext_sasl_ir_supported();
        self.authenticate(AuthenticateTask::plain(login, passwd, ir), id_params)
            .await
    }

    pub async fn authenticate_xoauth2(
        &mut self,
        user: &str,
        token: &str,
        id_params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        let ir = self.ext_sasl_ir_supported();
        self.authenticate(AuthenticateTask::xoauth2(user, token, ir), id_params)
            .await
    }

    pub async fn authenticate_oauthbearer(
        &mut self,
        a: &str,
        host: &str,
        port: u16,
        token: &str,
        id_params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        let ir = self.ext_sasl_ir_supported();
        self.authenticate(
            AuthenticateTask::oauthbearer(a, host, port, token, ir),
            id_params,
        )
        .await
    }

    pub async fn id(
        &mut self,
        params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        if self.ext_id_supported() {
            self.stream
                .progress(self.resolver.resolve(IdTask::new(params)))
                .await?
                .map_err(ClientError::resolve("id"))
        } else {
            warn!("IMAP ID extension not supported, skipping");
            Ok(Default::default())
        }
    }

    pub async fn create(&mut self, mailbox: Mailbox<'_>) -> Result<(), ClientError> {
        let mbox = mailbox.into_static();

        self.stream
            .progress(self.resolver.resolve(CreateTask::new(mbox)))
            .await?
            .map_err(ClientError::resolve("create"))
    }

    pub async fn list(
        &mut self,
        mailbox: Mailbox<'_>,
        mailbox_wildcard: ListMailbox<'_>,
    ) -> Result<
        Vec<(
            Mailbox<'static>,
            Option<QuotedChar>,
            Vec<FlagNameAttribute<'static>>,
        )>,
        ClientError,
    > {
        let mbox = mailbox.into_static();
        let mbox_wcard = mailbox_wildcard.into_static();

        self.stream
            .progress(self.resolver.resolve(ListTask::new(mbox, mbox_wcard)))
            .await?
            .map_err(ClientError::resolve("list"))
    }

    pub async fn select(
        &mut self,
        mailbox: Mailbox<'_>,
    ) -> Result<SelectDataUnvalidated, ClientError> {
        let mbox = mailbox.into_static();

        self.stream
            .progress(self.resolver.resolve(SelectTask::new(mbox)))
            .await?
            .map_err(ClientError::resolve("select"))
    }

    pub async fn examine(
        &mut self,
        mailbox: Mailbox<'_>,
    ) -> Result<SelectDataUnvalidated, ClientError> {
        let mbox = mailbox.into_static();

        self.stream
            .progress(self.resolver.resolve(SelectTask::read_only(mbox)))
            .await?
            .map_err(ClientError::resolve("examine"))
    }

    pub async fn expunge(&mut self) -> Result<Vec<NonZeroU32>, ClientError> {
        self.stream
            .progress(self.resolver.resolve(ExpungeTask::new()))
            .await?
            .map_err(ClientError::resolve("expunge"))
    }

    pub async fn delete(&mut self, mailbox: Mailbox<'_>) -> Result<(), ClientError> {
        let mbox = mailbox.into_static();

        self.stream
            .progress(self.resolver.resolve(DeleteTask::new(mbox)))
            .await?
            .map_err(ClientError::resolve("delete"))
    }

    pub async fn search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        let criteria: Vec<_> = criteria
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let criteria = if criteria.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(criteria).unwrap()
        };

        self.stream
            .progress(
                self.resolver
                    .resolve(SearchTask::new(criteria).with_uid(uid)),
            )
            .await?
            .map_err(ClientError::resolve("search"))
    }

    pub async fn sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        charset: Charset<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        let sort: Vec<_> = sort_criteria.into_iter().collect();
        let sort = if sort.is_empty() {
            Vec1::from(SortCriterion {
                reverse: true,
                key: SortKey::Date,
            })
        } else {
            Vec1::try_from(sort).unwrap()
        };

        let charset = charset.into_static();

        let search: Vec<_> = search_criteria
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();
        let search = if search.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(search).unwrap()
        };

        self.stream
            .progress(
                self.resolver.resolve(
                    SortTask::new(sort, search)
                        .with_uid(uid)
                        .with_charset(charset),
                ),
            )
            .await?
            .map_err(ClientError::resolve("sort"))
    }

    pub async fn sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        charset: Charset<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        let mut fetch_items = match fetch_items {
            MacroOrMessageDataItemNames::Macro(m) => m.expand().into_static(),
            MacroOrMessageDataItemNames::MessageDataItemNames(items) => items,
        };

        if uid {
            fetch_items.push(MessageDataItemName::Uid);
        }

        if self.ext_sort_supported() {
            let fetch_items = MacroOrMessageDataItemNames::MessageDataItemNames(fetch_items);

            let ids = self
                .sort(sort_criteria, charset, search_criteria, uid)
                .await?;

            let mut fetches = self
                .fetch(ids.clone().try_into().unwrap(), fetch_items, uid)
                .await?;

            let items = ids.into_iter().flat_map(|id| fetches.remove(&id)).collect();

            Ok(items)
        } else {
            warn!("IMAP SORT extension not supported, using fallback");
            let ids = self.search(search_criteria, uid).await?;

            sort_criteria
                .clone()
                .into_iter()
                .filter_map(|criterion| match criterion.key {
                    SortKey::Arrival => Some(MessageDataItemName::InternalDate),
                    SortKey::Cc => Some(MessageDataItemName::Envelope),
                    SortKey::Date => Some(MessageDataItemName::Envelope),
                    SortKey::From => Some(MessageDataItemName::Envelope),
                    SortKey::Size => Some(MessageDataItemName::Rfc822Size),
                    SortKey::Subject => Some(MessageDataItemName::Envelope),
                    SortKey::To => Some(MessageDataItemName::Envelope),
                    SortKey::DisplayFrom => None,
                    SortKey::DisplayTo => None,
                })
                .for_each(|item| {
                    if !fetch_items.contains(&item) {
                        fetch_items.push(item)
                    }
                });

            let mut fetches: Vec<_> = self
                .fetch(ids.try_into().unwrap(), fetch_items.into(), uid)
                .await?
                .into_values()
                .collect();

            fetches.sort_by(|a, b| {
                for criterion in sort_criteria.clone().into_iter() {
                    let mut cmp = cmp_fetch_items(&criterion.key, a, b);

                    if criterion.reverse {
                        cmp = cmp.reverse();
                    }

                    if cmp.is_ne() {
                        return cmp;
                    }
                }

                cmp_fetch_items(&SortKey::Date, a, b)
            });

            Ok(fetches)
        }
    }

    pub fn enqueue_idle(&mut self) -> Tag<'static> {
        let tag = self.resolver.scheduler.tag_generator.generate();

        self.resolver.scheduler.flow.enqueue_command(Command {
            tag: tag.clone(),
            body: CommandBody::Idle,
        });

        tag.into_static()
    }

    #[tracing::instrument(name = "idle", skip_all)]
    pub async fn idle(&mut self, tag: Tag<'static>) -> Result<(), StreamError<ClientFlowError>> {
        debug!("starting the main loop");

        loop {
            let progress = self.stream.progress(&mut self.resolver.scheduler.flow);
            match tokio::time::timeout(self.idle_timeout, progress).await {
                Err(_) => {
                    debug!("timed out, sending done command…");
                    self.resolver.scheduler.flow.set_idle_done();
                }
                Ok(Err(err)) => {
                    break Err(err);
                }
                Ok(Ok(ClientFlowEvent::IdleCommandSent { .. })) => {
                    debug!("command sent");
                }
                Ok(Ok(ClientFlowEvent::IdleAccepted { .. })) => {
                    debug!("command accepted, entering idle mode");
                }
                Ok(Ok(ClientFlowEvent::IdleRejected { status, .. })) => {
                    warn!("command rejected, aborting: {status:?}");
                    break Ok(());
                }
                Ok(Ok(ClientFlowEvent::IdleDoneSent { .. })) => {
                    debug!("done command sent");
                }
                Ok(Ok(ClientFlowEvent::DataReceived { data })) => {
                    debug!("received data, sending done command…");
                    trace!("{data:#?}");
                    self.resolver.scheduler.flow.set_idle_done();
                }
                Ok(Ok(ClientFlowEvent::StatusReceived {
                    status:
                        Status::Tagged(Tagged {
                            tag: ref got_tag, ..
                        }),
                })) if *got_tag == tag => {
                    debug!("received tagged response, exiting");
                    break Ok(());
                }
                Ok(event) => {
                    debug!("received unknown event, ignoring: {event:?}");
                }
            }
        }
    }

    #[tracing::instrument(name = "idle/done", skip_all)]
    pub async fn idle_done(
        &mut self,
        tag: Tag<'static>,
    ) -> Result<(), StreamError<ClientFlowError>> {
        self.resolver.scheduler.flow.set_idle_done();

        loop {
            let progress = self
                .stream
                .progress(&mut self.resolver.scheduler.flow)
                .await?;

            match progress {
                ClientFlowEvent::IdleDoneSent { .. } => {
                    debug!("done command sent");
                }
                ClientFlowEvent::StatusReceived {
                    status:
                        Status::Tagged(Tagged {
                            tag: ref got_tag, ..
                        }),
                } if *got_tag == tag => {
                    debug!("received tagged response, exiting");
                    break Ok(());
                }
                event => {
                    debug!("received unknown event, ignoring: {event:?}");
                }
            }
        }
    }

    pub async fn thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        charset: Charset<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<Thread>, ClientError> {
        let alg = algorithm.into_static();

        let charset = charset.into_static();

        let search: Vec<_> = search_criteria
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();
        let search = if search.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(search).unwrap()
        };

        self.stream
            .progress(
                self.resolver.resolve(
                    ThreadTask::new(alg, search)
                        .with_uid(uid)
                        .with_charset(charset),
                ),
            )
            .await?
            .map_err(ClientError::resolve("thread"))
    }

    pub async fn store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
        uid: bool,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        self.stream
            .progress(
                self.resolver
                    .resolve(StoreTask::new(sequence_set, kind, flags).with_uid(uid)),
            )
            .await?
            .map_err(ClientError::resolve("store"))
    }

    pub async fn silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        self.stream
            .progress(self.resolver.resolve(SilentStoreTask::new(
                StoreTask::new(sequence_set, kind, flags).with_uid(uid),
            )))
            .await?
            .map_err(ClientError::resolve("store"))
    }

    fn to_static_literal(
        &self,
        message: impl AsRef<[u8]>,
    ) -> Result<LiteralOrLiteral8<'static>, ValidationError> {
        let message = if self.ext_binary_supported() {
            LiteralOrLiteral8::Literal8(Literal8 {
                data: message.as_ref().into(),
                mode: LiteralMode::Sync,
            })
        } else {
            warn!("IMAP BINARY extension not supported, using fallback");
            Literal::validate(message.as_ref())?;
            LiteralOrLiteral8::Literal(Literal::unvalidated(message.as_ref()))
        };

        Ok(message.into_static())
    }

    pub async fn append(
        &mut self,
        mailbox: Mailbox<'_>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<u32>, ClientError> {
        let mbox = mailbox.into_static();

        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let msg = self.to_static_literal(message)?;

        self.stream
            .progress(
                self.resolver
                    .resolve(AppendTask::new(mbox, msg).with_flags(flags)),
            )
            .await?
            .map_err(ClientError::resolve("append"))
    }

    pub async fn post_append_noop(&mut self) -> Result<Option<u32>, ClientError> {
        self.stream
            .progress(self.resolver.resolve(PostAppendNoOpTask::new()))
            .await?
            .map_err(ClientError::resolve("noop"))
    }

    pub async fn post_append_check(&mut self) -> Result<Option<u32>, ClientError> {
        self.stream
            .progress(self.resolver.resolve(PostAppendCheckTask::new()))
            .await?
            .map_err(ClientError::resolve("check"))
    }

    pub async fn appenduid(
        &mut self,
        mailbox: Mailbox<'_>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<(NonZeroU32, NonZeroU32)>, ClientError> {
        let mbox = mailbox.into_static();

        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let msg = self.to_static_literal(message)?;

        self.stream
            .progress(
                self.resolver
                    .resolve(AppendUidTask::new(mbox, msg).with_flags(flags)),
            )
            .await?
            .map_err(ClientError::resolve("appenduid"))
    }

    pub async fn appenduid_or_fallback(
        &mut self,
        mailbox: Mailbox<'_>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<NonZeroU32>, ClientError> {
        if self.ext_uidplus_supported() {
            Ok(self
                .appenduid(mailbox, flags, message)
                .await?
                .map(|(uid, _)| uid))
        } else {
            warn!("IMAP UIDPLUS extension not supported, using fallback");

            // If the mailbox is currently selected, the normal new
            // message actions SHOULD occur.  Specifically, the server
            // SHOULD notify the client immediately via an untagged
            // EXISTS response.  If the server does not do so, the
            // client MAY issue a NOOP command (or failing that, a
            // CHECK command) after one or more APPEND commands.
            //
            // <https://datatracker.ietf.org/doc/html/rfc3501#section-6.3.11>
            self.select(mailbox.clone()).await?;

            let seq = match self.append(mailbox, flags, message).await? {
                Some(seq) => seq,
                None => match self.post_append_noop().await? {
                    Some(seq) => seq,
                    None => self
                        .post_append_check()
                        .await?
                        .ok_or(ClientError::ResolveTask(
                            TaskError::MissingData("APPENDUID: seq".into()),
                            "appenduid",
                        ))?,
                },
            };

            let uid = self
                .search(
                    Vec1::from(SearchKey::SequenceSet(seq.try_into().unwrap())),
                    true,
                )
                .await?
                .into_iter()
                .next();

            Ok(uid)
        }
    }

    pub async fn fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        let items = items.into_static();

        self.stream
            .progress(
                self.resolver
                    .resolve(FetchTask::new(sequence_set, items).with_uid(uid)),
            )
            .await?
            .map_err(ClientError::resolve("fetch"))
    }

    pub async fn fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        let items = items.into_static();

        self.stream
            .progress(
                self.resolver
                    .resolve(FetchFirstTask::new(id, items).with_uid(uid)),
            )
            .await?
            .map_err(ClientError::resolve("fetch"))
    }

    pub async fn copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: Mailbox<'_>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let task = CopyTask::new(sequence_set, mailbox.into_static()).with_uid(uid);

        self.stream
            .progress(self.resolver.resolve(task))
            .await?
            .map_err(ClientError::resolve("copy"))
    }

    pub async fn r#move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: Mailbox<'_>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let task = MoveTask::new(sequence_set, mailbox.into_static()).with_uid(uid);

        self.stream
            .progress(self.resolver.resolve(task))
            .await?
            .map_err(ClientError::resolve("move"))
    }

    pub async fn move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: Mailbox<'_>,
        uid: bool,
    ) -> Result<(), ClientError> {
        if self.ext_move_supported() {
            self.r#move(sequence_set, mailbox, uid).await
        } else {
            warn!("IMAP MOVE extension not supported, using fallback");
            self.copy(sequence_set.clone(), mailbox, uid).await?;
            self.silent_store(sequence_set, StoreType::Add, vec![Flag::Deleted], uid)
                .await?;
            self.expunge().await?;
            Ok(())
        }
    }

    pub async fn check(&mut self) -> Result<(), ClientError> {
        self.stream
            .progress(self.resolver.resolve(CheckTask::new()))
            .await?
            .map_err(ClientError::resolve("check"))
    }

    pub async fn noop(&mut self) -> Result<(), ClientError> {
        self.stream
            .progress(self.resolver.resolve(NoOpTask::new()))
            .await?
            .map_err(ClientError::resolve("noop"))
    }
}

fn cmp_fetch_items(
    criterion: &SortKey,
    a: &Vec1<MessageDataItem>,
    b: &Vec1<MessageDataItem>,
) -> Ordering {
    use MessageDataItem::*;

    match &criterion {
        SortKey::Arrival => {
            let a = a.as_ref().iter().find_map(|a| {
                if let InternalDate(dt) = a {
                    Some(dt.as_ref())
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let InternalDate(dt) = b {
                    Some(dt.as_ref())
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Date => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Envelope(envelope) = a {
                    envelope.date.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Envelope(envelope) = b {
                    envelope.date.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Size => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Rfc822Size(size) = a {
                    Some(size)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Rfc822Size(size) = b {
                    Some(size)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Subject => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Envelope(envelope) = a {
                    envelope.subject.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Envelope(envelope) = b {
                    envelope.subject.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        // FIXME: Address missing Ord derive in imap-types
        SortKey::Cc | SortKey::From | SortKey::To | SortKey::DisplayFrom | SortKey::DisplayTo => {
            Ordering::Equal
        }
    }
}
