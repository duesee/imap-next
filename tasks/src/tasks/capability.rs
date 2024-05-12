use imap_types::{
    command::CommandBody,
    core::Vec1,
    response::{Capability, Code, Data, StatusBody, StatusKind},
};

use crate::{tasks::TaskError, Task};

#[derive(Clone, Debug, Default)]
pub struct CapabilityTask {
    /// We use this as scratch space.
    output: Option<Vec1<Capability<'static>>>,
}

impl CapabilityTask {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Task for CapabilityTask {
    type Output = Result<Vec1<Capability<'static>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Capability
    }

    // Capabilities may be found in a data response.
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Capability(capabilities) = data {
            self.output = Some(capabilities);
            None
        } else {
            Some(data)
        }
    }

    // Capabilities may be found in the status body of an untagged response.
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
                    // Capabilities may also be found in the status body of tagged response.
                    if let Some(Code::Capability(capabilities)) = status_body.code {
                        Ok(capabilities)
                    } else {
                        Err(TaskError::MissingData("CAPABILITY".into()))
                    }
                }
            },
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
