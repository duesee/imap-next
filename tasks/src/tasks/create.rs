use imap_types::{
    command::CommandBody,
    mailbox::Mailbox,
    response::{StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type CreateTaskOutput = ();

#[derive(Clone, Debug)]
pub struct CreateTask {
    mailbox: Mailbox<'static>,
}

impl CreateTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(mailbox: Mailbox<'static>) -> Self {
        Self { mailbox }
    }
}

impl Task for CreateTask {
    type Output = Result<CreateTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Create {
            mailbox: self.mailbox.clone(),
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
