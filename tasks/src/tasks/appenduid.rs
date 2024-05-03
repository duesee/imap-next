use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    extensions::binary::LiteralOrLiteral8,
    flag::Flag,
    mailbox::Mailbox,
    response::{Code, StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type AppendUidTaskOutput = Option<NonZeroU32>;

#[derive(Clone, Debug)]
pub struct AppendUidTask {
    mailbox: Mailbox<'static>,
    flags: Vec<Flag<'static>>,
    message: LiteralOrLiteral8<'static>,
}

impl AppendUidTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        mailbox: Mailbox<'static>,
        flags: Vec<Flag<'static>>,
        message: LiteralOrLiteral8<'static>,
    ) -> Self {
        Self {
            mailbox,
            flags,
            message,
        }
    }
}

impl Task for AppendUidTask {
    type Output = Result<AppendUidTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Append {
            mailbox: self.mailbox.clone(),
            flags: self.flags.clone(),
            date: None,
            message: self.message.clone(),
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => {
                if let Some(Code::AppendUid { uid, .. }) = status_body.code {
                    Ok(Some(uid))
                } else {
                    Ok(None)
                }
            }
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
