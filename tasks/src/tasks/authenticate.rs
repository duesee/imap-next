use std::borrow::Cow;

use imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::CommandBody,
    core::Vec1,
    response::{Capability, Code, CommandContinuationRequest, Data, StatusBody, StatusKind},
    secret::Secret,
};

use crate::{tasks::TaskError, Task};

#[derive(Clone, Debug)]
pub struct AuthenticateTask {
    /// Authentication mechanism.
    ///
    /// Note: Currently only used for `AUTH=PLAIN`.
    ///       Invariants need to be enforced through constructors.
    mechanism: AuthMechanism<'static>,
    /// Static authentication data.
    ///
    /// Note: Currently only used for `AUTH=PLAIN`.
    line: Option<Vec<u8>>,
    /// Does the server support SASL's initial response?
    ir: bool,
    output: Option<Vec1<Capability<'static>>>,
}

impl AuthenticateTask {
    pub fn plain(login: &str, passwd: &str, ir: bool) -> Self {
        let line = format!("\x00{login}\x00{passwd}");

        Self {
            mechanism: AuthMechanism::Plain,
            line: Some(line.into_bytes()),
            ir,
            output: None,
        }
    }
}

impl Task for AuthenticateTask {
    type Output = Result<Option<Vec1<Capability<'static>>>, TaskError>;

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

    // Capabilities may (unfortunately) be found in a data response.
    // See https://github.com/modern-email/defects/issues/18
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
                // Capabilities may be found in the status body of tagged response.
                if let Some(Code::Capability(capabilities)) = status_body.code {
                    Some(capabilities)
                } else {
                    None
                },
            )),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
