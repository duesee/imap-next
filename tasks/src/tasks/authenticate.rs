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
    line: Option<Vec<u8>>,
    ir: bool,
    output: AuthenticateTaskOutput,
}

impl AuthenticateTask {
    #[cfg_attr(debug_assertions, tracing::instrument(skip(passwd)))]
    pub fn new_plain(login: &str, passwd: &str, ir: bool) -> Self {
        let line = format!("\x00{login}\x00{passwd}");

        Self {
            mechanism: AuthMechanism::Plain,
            line: Some(line.into_bytes()),
            ir,
            output: None,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(token)))]
    pub fn new_xoauth2(user: &str, token: &str, ir: bool) -> Self {
        let line = format!("user={user}\x01auth=Bearer {token}\x01\x01");

        Self {
            mechanism: AuthMechanism::XOAuth2,
            line: Some(line.into_bytes()),
            ir,
            output: None,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(token)))]
    pub fn new_oauth_bearer(a: &str, host: &str, port: u16, token: &str, ir: bool) -> Self {
        let line = format!("n,a={a},\x01host={host}\x01port={port}\x01auth=Bearer {token}\x01\x01");

        Self {
            mechanism: AuthMechanism::XOAuth2,
            line: Some(line.into_bytes()),
            ir,
            output: None,
        }
    }
}

impl Task for AuthenticateTask {
    type Output = Result<AuthenticateTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
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

    /// Process data response.
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
    fn process_continuation_request_authenticate(
        &mut self,
        _: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData<'static>, CommandContinuationRequest<'static>> {
        match self.mechanism {
            // <https://developers.google.com/gmail/imap/xoauth2-protocol>
            AuthMechanism::XOAuth2 => {
                // SASL-IR is supported, so line was already
                // sent. Therefore, the current continuation request
                // indicates an error.
                //
                // > The client sends an empty response ("\r\n") to
                // the challenge containing the error message.
                if self.ir {
                    Ok(AuthenticateData::r#continue(Vec::with_capacity(0)))
                } else
                // SASL-IR is not supported, so the line needs to be
                // sent.
                if let Some(line) = self.line.take() {
                    Ok(AuthenticateData::r#continue(line))
                } else {
                    Ok(AuthenticateData::Cancel)
                }
            }
            _ => {
                if self.ir {
                    Ok(AuthenticateData::Cancel)
                } else if let Some(line) = self.line.take() {
                    Ok(AuthenticateData::r#continue(line))
                } else {
                    Ok(AuthenticateData::Cancel)
                }
            }
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output.or_else(|| {
                if let Some(Code::Capability(capabilities)) = status_body.code {
                    Some(capabilities)
                } else {
                    None
                }
            })),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
