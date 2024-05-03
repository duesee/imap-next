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
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new() -> Self {
        Default::default()
    }
}

impl Task for CapabilityTask {
    type Output = Result<Capabilities, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Capability
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Capability(capabilities) = data {
            self.output = Some(capabilities);
            None
        } else {
            Some(data)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
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

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
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
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
