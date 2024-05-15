use imap_types::{
    command::CommandBody,
    response::{StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug, Default)]
pub struct StartTlsTask;

impl Task for StartTlsTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::StartTLS
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl StartTlsTask {
    pub fn new() -> Self {
        Default::default()
    }
}
