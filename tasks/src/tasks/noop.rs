use imap_types::{
    command::CommandBody,
    response::{StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type NoOpTaskOutput = ();

#[derive(Clone, Debug)]
pub struct NoOpTask;

impl NoOpTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new() -> Self {
        Self
    }
}

impl Task for NoOpTask {
    type Output = Result<NoOpTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Noop
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
