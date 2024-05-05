use imap_types::{
    command::CommandBody,
    response::{StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type NoOpTaskOutput = ();

#[derive(Clone, Debug)]
pub struct NoOpTask;

impl NoOpTask {
    pub fn new() -> Self {
        Self
    }
}

impl Task for NoOpTask {
    type Output = Result<NoOpTaskOutput, SchedulerError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Noop
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(SchedulerError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(SchedulerError::UnexpectedBadResponse(status_body)),
        }
    }
}
