use std::borrow::Cow;

use imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::CommandBody,
    core::Vec1,
    response::{Capability, Code, CommandContinuationRequest, Data, StatusBody, StatusKind},
    secret::Secret,
    utils::escape_byte_string,
};
use tracing::error;

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct AuthenticateTask {
    /// Authentication mechanism.
    ///
    /// Note: Currently used for `AUTH=PLAIN`, `AUTH=OAUTHBEARER` and `AUTH=XOAUTH2`.
    ///       Invariants need to be enforced through constructors.
    mechanism: AuthMechanism<'static>,
    /// Static authentication data.
    ///
    /// Note: Currently used for `AUTH=PLAIN`, `AUTH=OAUTHBEARER` and `AUTH=XOAUTH2`.
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

    pub fn oauthbearer(user: &str, host: &str, port: u16, token: &str, ir: bool) -> Self {
        let line =
            format!("n,a={user},\x01host={host}\x01port={port}\x01auth=Bearer {token}\x01\x01");

        Self {
            mechanism: AuthMechanism::OAuthBearer,
            line: Some(line.into_bytes()),
            ir,
            output: None,
        }
    }

    pub fn xoauth2(user: &str, token: &str, ir: bool) -> Self {
        let line = format!("user={user}\x01auth=Bearer {token}\x01\x01");

        Self {
            mechanism: AuthMechanism::XOAuth2,
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
        continuation: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData<'static>, CommandContinuationRequest<'static>> {
        let cancel = || match self.mechanism {
            AuthMechanism::XOAuth2 => {
                let err = match continuation {
                    CommandContinuationRequest::Basic(data) => data.text().to_string(),
                    CommandContinuationRequest::Base64(data) => escape_byte_string(data.as_ref()),
                };

                error!("cannot authenticate using XOAUTH2 mechanism: {err}");

                AuthenticateData::r#continue(vec![])
            }
            _ => AuthenticateData::Cancel,
        };

        if self.ir {
            return Ok(cancel());
        }

        match self.line.take() {
            Some(data) => Ok(AuthenticateData::r#continue(data)),
            None => Ok(cancel()),
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(
                // Capabilities may be found in the status body of tagged response.
                if let Some(Code::Capability(capabilities)) = status_body.code {
                    Some(capabilities)
                } else {
                    self.output
                },
            ),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
