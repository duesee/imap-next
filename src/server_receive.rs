use imap_codec::{AuthenticateDataCodec, CommandCodec, IdleDoneCodec};

use crate::receive::ReceiveState;

pub enum ServerReceiveState {
    Command(ReceiveState<CommandCodec>),
    AuthenticateData(ReceiveState<AuthenticateDataCodec>),
    IdleAccept(ReceiveState<NoCodec>),
    IdleDone(ReceiveState<IdleDoneCodec>),
    // This state is set only temporarily during `ServerReceiveState::change_state`
    Dummy,
}

impl ServerReceiveState {
    pub fn change_state(&mut self, next_expected_message: NextExpectedMessage) {
        // NOTE: This function MUST NOT panic. Otherwise the dummy state will remain indefinitely.
        let old_state = std::mem::replace(self, ServerReceiveState::Dummy);
        let new_state = match next_expected_message {
            NextExpectedMessage::Command => {
                let codec = CommandCodec::default();
                Self::Command(match old_state {
                    Self::Command(state) => state,
                    Self::AuthenticateData(state) => state.change_codec(codec),
                    Self::IdleAccept(state) => state.change_codec(codec),
                    Self::IdleDone(state) => state.change_codec(codec),
                    Self::Dummy => unreachable!(),
                })
            }
            NextExpectedMessage::AuthenticateData => {
                let codec = AuthenticateDataCodec::default();
                Self::AuthenticateData(match old_state {
                    Self::Command(state) => state.change_codec(codec),
                    Self::AuthenticateData(state) => state,
                    Self::IdleAccept(state) => state.change_codec(codec),
                    Self::IdleDone(state) => state.change_codec(codec),
                    Self::Dummy => unreachable!(),
                })
            }
            NextExpectedMessage::IdleAccept => {
                let codec = NoCodec;
                Self::IdleAccept(match old_state {
                    Self::Command(state) => state.change_codec(codec),
                    Self::AuthenticateData(state) => state.change_codec(codec),
                    Self::IdleAccept(state) => state,
                    Self::IdleDone(state) => state.change_codec(codec),
                    Self::Dummy => unreachable!(),
                })
            }
            NextExpectedMessage::IdleDone => {
                let codec = IdleDoneCodec::default();
                Self::IdleDone(match old_state {
                    Self::Command(state) => state.change_codec(codec),
                    Self::AuthenticateData(state) => state.change_codec(codec),
                    Self::IdleAccept(state) => state.change_codec(codec),
                    Self::IdleDone(state) => state,
                    Self::Dummy => unreachable!(),
                })
            }
        };
        *self = new_state;
    }
}

#[derive(Clone, Copy)]
pub enum NextExpectedMessage {
    Command,
    AuthenticateData,
    IdleAccept,
    IdleDone,
}

/// Dummy codec used for technical reasons when we don't want to receive anything at all.
pub struct NoCodec;
