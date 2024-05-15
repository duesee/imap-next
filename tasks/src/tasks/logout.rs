use imap_types::{
    command::CommandBody,
    response::{Bye, StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug, Default)]
pub struct LogoutTask {
    got_bye: bool,
}

impl Task for LogoutTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Logout
    }

    fn process_bye(&mut self, _: Bye<'static>) -> Option<Bye<'static>> {
        self.got_bye = true;
        None
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => {
                if self.got_bye {
                    Ok(())
                } else {
                    Err(TaskError::MissingData("LOGOUT: BYE".into()))
                }
            }
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl LogoutTask {
    pub fn new() -> Self {
        Default::default()
    }
}
