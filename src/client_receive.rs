use imap_codec::{GreetingCodec, ResponseCodec};

use crate::receive::ReceiveState;

pub enum ClientReceiveState {
    Greeting(ReceiveState<GreetingCodec>),
    Response(ReceiveState<ResponseCodec>),
    // This state is set only temporarily during `ClientReceiveState::change_state`
    Dummy,
}

impl ClientReceiveState {
    pub fn change_state(&mut self) {
        // NOTE: This function MUST NOT panic. Otherwise the dummy state will remain indefinitely.
        let old_state = std::mem::replace(self, ClientReceiveState::Dummy);
        let codec = ResponseCodec::default();
        let new_state = Self::Response(match old_state {
            Self::Greeting(state) => state.change_codec(codec),
            Self::Response(state) => state,
            Self::Dummy => unreachable!(),
        });
        *self = new_state;
    }
}
