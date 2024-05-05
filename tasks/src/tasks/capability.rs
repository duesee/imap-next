use imap_types::{
    command::CommandBody,
    core::Vec1,
    response::{Capability, Code, Data, StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type CapabilityTaskOutput = Vec1<Capability<'static>>;
pub type Capabilities = CapabilityTaskOutput;

#[derive(Clone, Debug, Default)]
pub struct CapabilityTask {
    output: Option<Capabilities>,
}

impl CapabilityTask {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Task for CapabilityTask {
    type Output = Result<Capabilities, SchedulerError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Capability
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Capability(capabilities) = data {
            self.output = Some(capabilities);
            None
        } else {
            Some(data)
        }
    }

    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        if let Some(Code::Capability(capabilities)) = status_body.code {
            self.output = Some(capabilities);
            None
        } else {
            Some(status_body)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => match self.output {
                Some(capabilities) => Ok(capabilities),
                None => {
                    if let Some(Code::Capability(capabilities)) = status_body.code {
                        Ok(capabilities)
                    } else {
                        Err(SchedulerError::MissingData("CAPABILITY".into()))
                    }
                }
            },
            StatusKind::No => Err(SchedulerError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(SchedulerError::UnexpectedBadResponse(status_body)),
        }
    }
}
