use imap_types::{
    command::CommandBody,
    mailbox::Mailbox,
    response::{StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct CreateTask {
    mailbox: Mailbox<'static>,
}

impl CreateTask {
    pub fn new(mailbox: Mailbox<'static>) -> Self {
        Self { mailbox }
    }
}

impl Task for CreateTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Create {
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
