use std::borrow::Cow;

use imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::CommandBody,
    response::{CommandContinuationRequest, StatusBody},
    secret::Secret,
};

use crate::Task;

pub struct AuthenticatePlainTask {
    line: Option<Vec<u8>>,
    ir: bool,
}

impl AuthenticatePlainTask {
    pub fn new(username: &str, password: &str, ir: bool) -> Self {
        Self {
            line: Some(format!("\x00{}\x00{}", username, password).into_bytes()),
            ir,
        }
    }
}

impl Task for AuthenticatePlainTask {
    type Output = Result<(), ()>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Authenticate {
            mechanism: AuthMechanism::Plain,
            initial_response: if self.ir {
                // TODO: command_body must only be called once... hm...
                Some(Secret::new(Cow::Owned(self.line.clone().unwrap())))
            } else {
                None
            },
        }
    }

    fn process_continuation_request_authenticate(
        &mut self,
        _: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData, CommandContinuationRequest<'static>> {
        if self.ir {
            Ok(AuthenticateData::Cancel)
        } else if let Some(line) = self.line.take() {
            Ok(AuthenticateData::r#continue(line))
        } else {
            Ok(AuthenticateData::Cancel)
        }
    }

    fn process_tagged(self, _: StatusBody<'static>) -> Self::Output {
        Ok(())
    }
}
