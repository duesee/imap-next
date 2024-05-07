use imap_types::{
    command::CommandBody,
    response::{Bye, StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type LogoutTaskOutput = ();

#[derive(Clone, Debug, Default)]
pub struct LogoutTask {
    got_bye: bool,
}

impl Task for LogoutTask {
    type Output = Result<LogoutTaskOutput, SchedulerError>;

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
                    Err(SchedulerError::MissingData("LOGOUT: BYE".into()))
                }
            }
            StatusKind::No => Err(SchedulerError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(SchedulerError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl LogoutTask {
    pub fn new() -> Self {
        Default::default()
    }
}
