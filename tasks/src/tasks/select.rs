use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    flag::{Flag, FlagPerm},
    mailbox::Mailbox,
    response::{Code, Data, StatusBody, StatusKind},
};
use tracing::{error, warn};

use crate::{SchedulerError, Task};

#[derive(Clone, Debug, Default)]
pub struct SelectTaskOutput {
    // required untagged responses
    pub flags: Option<Vec<Flag<'static>>>,
    pub exists: Option<u32>,
    pub recent: Option<u32>,

    // required OK untagged responses
    pub unseen: Option<NonZeroU32>,
    pub permanent_flags: Option<Vec<FlagPerm<'static>>>,
    pub uid_next: Option<NonZeroU32>,
    pub uid_validity: Option<NonZeroU32>,
}

impl SelectTaskOutput {
    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    pub fn validate(self) -> Result<Self, SchedulerError> {
        // TODO: make strictness behaviour customizable?

        if self.flags.is_none() {
            warn!("missing required FLAGS untagged response");
        }
        if self.exists.is_none() {
            warn!("missing required EXISTS untagged response");
        }
        if self.recent.is_none() {
            warn!("missing required RECENT untagged response");
        }

        if self.unseen.is_none() {
            warn!("missing required UNSEEN OK untagged response");
        }
        if self.permanent_flags.is_none() {
            warn!("missing required PERMANENTFLAGS OK untagged response");
        }
        if self.uid_next.is_none() {
            warn!("missing required UIDNEXT OK untagged response");
        }
        if self.uid_validity.is_none() {
            warn!("missing required UIDVALIDITY OK untagged response");
        }

        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct SelectTask {
    mailbox: Mailbox<'static>,
    read_only: bool,
    output: SelectTaskOutput,
}

impl SelectTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(mailbox: Mailbox<'static>, read_only: bool) -> Self {
        Self {
            mailbox,
            read_only,
            output: Default::default(),
        }
    }
}

impl Task for SelectTask {
    type Output = Result<SelectTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        let mailbox = self.mailbox.clone();
        if self.read_only {
            CommandBody::Examine { mailbox }
        } else {
            CommandBody::Select { mailbox }
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        match data {
            Data::Flags(flags) => {
                self.output.flags = Some(flags);
                None
            }
            Data::Exists(count) => {
                self.output.exists = Some(count);
                None
            }
            Data::Recent(count) => {
                self.output.recent = Some(count);
                None
            }
            data => Some(data),
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        let StatusBody { kind, code, text } = status_body;

        match code {
            Some(Code::Unseen(seq)) => match &kind {
                StatusKind::Ok => {
                    self.output.unseen = Some(seq);
                    None
                }
                StatusKind::No => {
                    warn!("{text}");
                    Some(StatusBody { kind, code, text })
                }
                StatusKind::Bad => {
                    error!("{text}");
                    Some(StatusBody { kind, code, text })
                }
            },
            Some(Code::PermanentFlags(flags)) => match &kind {
                StatusKind::Ok => {
                    self.output.permanent_flags = Some(flags);
                    None
                }
                StatusKind::No => {
                    warn!("{text}");
                    let code = Some(Code::PermanentFlags(flags));
                    Some(StatusBody { kind, code, text })
                }
                StatusKind::Bad => {
                    error!("{text}");
                    let code = Some(Code::PermanentFlags(flags));
                    Some(StatusBody { kind, code, text })
                }
            },
            Some(Code::UidNext(uid)) => match &kind {
                StatusKind::Ok => {
                    self.output.uid_next = Some(uid);
                    None
                }
                StatusKind::No => {
                    warn!("{text}");
                    Some(StatusBody { kind, code, text })
                }
                StatusKind::Bad => {
                    error!("{text}");
                    Some(StatusBody { kind, code, text })
                }
            },
            Some(Code::UidValidity(uid)) => match &kind {
                StatusKind::Ok => {
                    self.output.uid_validity = Some(uid);
                    None
                }
                StatusKind::No => {
                    warn!("{text}");
                    Some(StatusBody { kind, code, text })
                }
                StatusKind::Bad => {
                    error!("{text}");
                    Some(StatusBody { kind, code, text })
                }
            },
            _ => Some(StatusBody { kind, code, text }),
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => self.output.validate(),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
