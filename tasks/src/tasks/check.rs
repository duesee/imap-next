use imap_types::{
    command::CommandBody,
    response::{StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug, Default)]
pub struct CheckTask;

impl Task for CheckTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Check
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl CheckTask {
    pub fn new() -> Self {
        Default::default()
    }
}
