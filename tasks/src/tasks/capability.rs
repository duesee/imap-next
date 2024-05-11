use imap_types::{
    command::CommandBody,
    core::Vec1,
    response::{Capability, Data, StatusBody, StatusKind},
};

use crate::Task;

#[derive(Default)]
pub struct CapabilityTask {
    /// We use this as scratch space.
    capabilities: Option<Vec1<Capability<'static>>>,
}

impl Task for CapabilityTask {
    type Output = Result<Vec1<Capability<'static>>, &'static str>;

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
