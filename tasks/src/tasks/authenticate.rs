use std::borrow::Cow;

use imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::CommandBody,
    response::{Code, CommandContinuationRequest, Data, StatusBody, StatusKind},
    secret::Secret,
};

use crate::{SchedulerError, Task};

use super::capability::Capabilities;

pub type AuthenticateTaskOutput = Option<Capabilities>;

#[derive(Clone, Debug)]
pub struct AuthenticateTask {
    mechanism: AuthMechanism<'static>,
    /// The initial line used by the `SASL-IR` extension.
    line: Option<Vec<u8>>,
    /// Initial response from the `SASL-IR` extension.
    ir: bool,
    output: AuthenticateTaskOutput,
}

impl Task for AuthenticateTask {
    type Output = Result<AuthenticateTaskOutput, SchedulerError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Authenticate {
            mechanism: self.mechanism.clone(),
            initial_response: if self.ir {
                // TODO: command_body must only be called once... hm...
                Some(Secret::new(Cow::Owned(self.line.clone().unwrap())))
            } else {
                None
            },
        }
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

    fn process_continuation_request_authenticate(
        &mut self,
        _: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData<'static>, CommandContinuationRequest<'static>> {
        if self.ir {
            Ok(AuthenticateData::Cancel)
        } else if let Some(line) = self.line.take() {
            Ok(AuthenticateData::r#continue(line))
        } else {
            Ok(AuthenticateData::Cancel)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output.or(
                // Capabilities may also be found in the status body of tagged response.
                if let Some(Code::Capability(capabilities)) = status_body.code {
                    Some(capabilities)
                } else {
                    None
                },
            )),
            StatusKind::No => Err(SchedulerError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(SchedulerError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl AuthenticateTask {
    pub fn new_plain(login: &str, passwd: &str, ir: bool) -> Self {
        let line = format!("\x00{login}\x00{passwd}");

        Self {
            mechanism: AuthMechanism::Plain,
            line: Some(line.into_bytes()),
            ir,
            output: None,
        }
    }
}
