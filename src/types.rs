//! Types that extend `imap-types`.
// TODO: Do we really need this?

use std::borrow::Cow;

use imap_codec::imap_types::{
    auth::AuthMechanism,
    command::{Command, CommandBody},
    core::Tag,
    secret::Secret,
};

#[derive(Debug)]
pub struct CommandAuthenticate {
    pub tag: Tag<'static>,
    pub mechanism: AuthMechanism<'static>,
    pub initial_response: Option<Secret<Cow<'static, [u8]>>>,
}

impl From<CommandAuthenticate> for Command<'static> {
    fn from(command_authenticate: CommandAuthenticate) -> Self {
        Self {
            tag: command_authenticate.tag,
            body: CommandBody::Authenticate {
                mechanism: command_authenticate.mechanism,
                initial_response: command_authenticate.initial_response,
            },
        }
    }
}
