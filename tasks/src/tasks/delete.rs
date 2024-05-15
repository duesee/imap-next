use imap_types::{
    command::CommandBody,
    mailbox::Mailbox,
    response::{StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct DeleteTask {
    mailbox: Mailbox<'static>,
}

impl Task for DeleteTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Delete {
            mailbox: self.mailbox.clone(),
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl DeleteTask {
    pub fn new(mailbox: Mailbox<'static>) -> Self {
        Self { mailbox }
    }
}
