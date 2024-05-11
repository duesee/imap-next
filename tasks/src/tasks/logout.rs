use imap_types::{
    command::CommandBody,
    response::{Bye, StatusBody, StatusKind},
};

use crate::Task;

#[derive(Default)]
pub struct LogoutTask {
    got_bye: bool,
}

impl Task for LogoutTask {
    type Output = Result<(), &'static str>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Logout
    }

    fn process_bye(&mut self, _: Bye<'static>) -> Option<Bye<'static>> {
        self.got_bye = true;
        None
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => match self.got_bye {
                true => Ok(()),
                false => Err("missing REQUIRED untagged BYE"),
            },
            StatusKind::No => Err("unexpected NO result"),
            StatusKind::Bad => Err("command unknown or arguments invalid"),
        }
    }
}
