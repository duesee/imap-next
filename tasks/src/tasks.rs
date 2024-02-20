//! Collection of common IMAP tasks.
//!
//! The tasks here correspond to the invocation (and processing) of a single command.

use std::{borrow::Cow, fmt::Debug, num::NonZeroU32};

use imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::CommandBody,
    core::Vec1,
    fetch::{MacroOrMessageDataItemNames, MessageDataItem},
    flag::{Flag, FlagPerm},
    mailbox::Mailbox,
    response::{Bye, Capability, Code, CommandContinuationRequest, Data, StatusBody, StatusKind},
    secret::Secret,
    sequence::SequenceSet,
};

use crate::Task;

#[derive(Default)]
pub struct CapabilityTask {
    /// We use this as scratch space.
    capabilities: Option<Vec1<Capability<'static>>>,
}

impl Task for CapabilityTask {
    type Output = Result<Vec1<Capability<'static>>, &'static str>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Capability
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        match data {
            Data::Capability(capabilities) => {
                self.capabilities = Some(capabilities);
                None
            }
            unknown => Some(unknown),
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => match self.capabilities {
                Some(capabilities) => Ok(capabilities),
                None => Err("missing REQUIRED untagged CAPABILITY"),
            },
            StatusKind::No => Err("unexpected NO result"),
            StatusKind::Bad => Err("command unknown or arguments invalid"),
        }
    }
}

#[derive(Default)]
pub struct LoginTask {
    // Arguments: username password
    username: String,
    password: String,
}

impl LoginTask {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }
}

impl Task for LoginTask {
    // Responses: no specific responses for this command
    //
    // Result: OK - login completed, now in authenticated state
    //         NO - login failure: user name or password rejected
    //         BAD - command unknown or arguments invalid
    type Output = StatusBody<'static>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::login(self.username.clone(), self.password.clone()).unwrap()
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        status_body
    }
}

pub struct SelectTask {
    // Arguments: mailbox name
    mailbox: Mailbox<'static>,
    // Scratch space
    session: Session,
}

// Responses: REQUIRED untagged responses: FLAGS, EXISTS, RECENT
//            REQUIRED OK untagged responses: UNSEEN, PERMANENTFLAGS, UIDNEXT, UIDVALIDITY
#[derive(Debug, Default)]
pub struct Session {
    flags: Option<Vec<Flag<'static>>>,
    exists: Option<u32>,
    recent: Option<u32>,
    unseen: Option<NonZeroU32>,
    permanent_flags: Option<Vec<FlagPerm<'static>>>,
    uid_next: Option<NonZeroU32>,
    uid_validity: Option<NonZeroU32>,
}

impl SelectTask {
    pub fn new(mailbox: Mailbox<'static>) -> Self {
        Self {
            mailbox,
            session: Session::default(),
        }
    }
}

impl Task for SelectTask {
    // Result: OK - select completed, now in selected state
    //         NO - select failure, now in authenticated state: no such mailbox, can't access mailbox
    //         BAD - command unknown or arguments invalid
    type Output = (Session, StatusBody<'static>);

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::select(self.mailbox.clone()).unwrap()
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        match data {
            Data::Flags(flags) => self.session.flags = Some(flags),
            Data::Exists(exists) => self.session.exists = Some(exists),
            Data::Recent(recent) => self.session.recent = Some(recent),
            data => return Some(data),
        }

        None
    }

    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        if status_body.kind != StatusKind::Ok {
            return Some(status_body);
        }

        match status_body.code {
            Some(Code::PermanentFlags(permanent_flags)) => {
                self.session.permanent_flags = Some(permanent_flags)
            }
            Some(Code::UidNext(uid_next)) => self.session.uid_next = Some(uid_next),
            Some(Code::UidValidity(uid_validity)) => self.session.uid_validity = Some(uid_validity),
            Some(Code::Unseen(unseen)) => self.session.unseen = Some(unseen),
            _ => return Some(status_body),
        }

        None
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        (self.session, status_body)
    }
}

pub struct FetchTask {
    // Arguments: sequence set
    //            message data item names or macro
    sequence_set: SequenceSet,
    macro_or_item_names: MacroOrMessageDataItemNames<'static>,
    uid: bool,
    // Responses: untagged responses: FETCH
    fetches: Vec<Fetch>,
}

#[derive(Debug)]
pub struct Fetch {
    pub seq: NonZeroU32,
    pub items: Vec1<MessageDataItem<'static>>,
}

impl FetchTask {
    pub fn new<S, I>(sequence_set: S, macro_or_item_names: I, uid: bool) -> Self
    where
        S: TryInto<SequenceSet>,
        // TODO
        S::Error: Debug,
        I: Into<MacroOrMessageDataItemNames<'static>>,
    {
        Self {
            // TODO
            sequence_set: sequence_set.try_into().unwrap(),
            // TODO
            macro_or_item_names: macro_or_item_names.try_into().unwrap(),
            uid,
            fetches: vec![],
        }
    }
}

impl Task for FetchTask {
    // Result: OK - fetch completed
    //         NO - fetch error: can't fetch that data
    //         BAD - command unknown or arguments invalid
    type Output = (Vec<Fetch>, StatusBody<'static>);

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::fetch(
            self.sequence_set.clone(),
            self.macro_or_item_names.clone(),
            self.uid,
        )
        .unwrap()
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        match data {
            Data::Fetch { seq, items } => self.fetches.push(Fetch { seq, items }),
            data => return Some(data),
        }

        None
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        (self.fetches, status_body)
    }
}

#[derive(Default)]
pub struct LogoutTask {
    // Arguments: none
    // Responses:  REQUIRED untagged response: BYE
    bye: Option<Bye<'static>>,
}

impl LogoutTask {
    pub fn new() -> Self {
        Self { bye: None }
    }
}

impl Task for LogoutTask {
    // Result: OK - logout completed
    //         BAD - command unknown or arguments invalid
    type Output = (Option<Bye<'static>>, StatusBody<'static>);

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Logout
    }

    fn process_bye(&mut self, bye: Bye<'static>) -> Option<Bye<'static>> {
        self.bye = Some(bye);
        None
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        (self.bye, status_body)
    }
}

pub struct AuthenticatePlainTask {
    line: Option<Vec<u8>>,
    ir: bool,
}

impl AuthenticatePlainTask {
    pub fn new(username: &str, password: &str, ir: bool) -> Self {
        Self {
            line: Some(format!("\x00{}\x00{}", username, password).into_bytes()),
            ir,
        }
    }
}

impl Task for AuthenticatePlainTask {
    type Output = Result<(), ()>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Authenticate {
            mechanism: AuthMechanism::Plain,
            initial_response: if self.ir {
                // TODO: command_body must only be called once... hm...
                Some(Secret::new(Cow::Owned(self.line.clone().unwrap())))
            } else {
                None
            },
        }
    }

    fn process_continuation_authenticate(
        &mut self,
        _: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData, CommandContinuationRequest<'static>> {
        if self.ir {
            Ok(AuthenticateData::Cancel)
        } else if let Some(line) = self.line.take() {
            Ok(AuthenticateData::r#continue(line))
        } else {
            Ok(AuthenticateData::Cancel)
        }
    }

    fn process_tagged(self, _: StatusBody<'static>) -> Self::Output {
        Ok(())
    }
}
