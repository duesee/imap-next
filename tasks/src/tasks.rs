//! Collection of common IMAP tasks.
//!
//! The tasks here correspond to the invocation (and processing) of a single command.

use imap_codec::imap_types::{
    command::CommandBody,
    core::NonEmptyVec,
    response::{Bye, Capability, Data, StatusBody, StatusKind},
};

use crate::Task;

#[derive(Default)]
pub struct CapabilityTask {
    /// We use this as scratch space.
    capabilities: Option<NonEmptyVec<Capability<'static>>>,
}

impl Task for CapabilityTask {
    type Output = Result<NonEmptyVec<Capability<'static>>, &'static str>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Capability
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        match data {
            Data::Capability(capabilities) => {
                self.capabilities = Some(capabilities);
                None
            }
            unknown => Some(unknown),
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => match self.capabilities {
                Some(capabilities) => Ok(capabilities),
                None => Err("missing REQUIRED untagged CAPABILITY"),
            },
            StatusKind::No => Err("unexpected NO result"),
            StatusKind::Bad => Err("command unknown or arguments invalid"),
        }
    }
}

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
