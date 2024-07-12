use std::{collections::VecDeque, convert::Infallible};

use imap_codec::{
    encode::{Encoded, Encoder, Fragment},
    imap_types::response::{Greeting, Response},
    GreetingCodec, ResponseCodec,
};

use crate::{server::ResponseHandle, Interrupt, Io};

pub struct ServerSendState {
    greeting_codec: GreetingCodec,
    response_codec: ResponseCodec,
    // FIFO queue for messages that should be sent next.
    queued_messages: VecDeque<QueuedMessage>,
    // The message that is currently being sent.
    current_message: Option<CurrentMessage>,
}

impl ServerSendState {
    pub fn new(greeting_codec: GreetingCodec, response_codec: ResponseCodec) -> Self {
        Self {
            greeting_codec,
            response_codec,
            queued_messages: VecDeque::new(),
            current_message: None,
        }
    }

    pub fn enqueue_greeting(&mut self, greeting: Greeting<'static>) {
        self.queued_messages
            .push_back(QueuedMessage::Greeting { greeting });
    }

    pub fn enqueue_response(
        &mut self,
        handle: Option<ResponseHandle>,
        response: Response<'static>,
    ) {
        self.queued_messages
            .push_back(QueuedMessage::Response { handle, response });
    }

    pub fn next(&mut self) -> Result<Option<ServerSendEvent>, Interrupt<Infallible>> {
        match self.current_message.take() {
            Some(current_message) => {
                // Continue the message that was interrupted.
                let event = match current_message {
                    CurrentMessage::Greeting { greeting } => ServerSendEvent::Greeting { greeting },
                    CurrentMessage::Response { handle, response } => {
                        ServerSendEvent::Response { handle, response }
                    }
                };
                Ok(Some(event))
            }
            None => {
                let Some(queued_message) = self.queued_messages.pop_front() else {
                    // There is currently no message that needs to be sent
                    return Ok(None);
                };

                // Creates a buffer for writing the current message
                let mut write_buffer = Vec::new();

                // Push the bytes of the message to the buffer
                let current_message = queued_message.push_to_buffer(
                    &mut write_buffer,
                    &self.greeting_codec,
                    &self.response_codec,
                );

                self.current_message = Some(current_message);

                // Interrupt the state for sending all bytes of current message
                Err(Interrupt::Io(Io::Output(write_buffer)))
            }
        }
    }
}

/// Message that is queued but not sent yet.
enum QueuedMessage {
    Greeting {
        greeting: Greeting<'static>,
    },
    Response {
        handle: Option<ResponseHandle>,
        response: Response<'static>,
    },
}

impl QueuedMessage {
    fn push_to_buffer(
        self,
        write_buffer: &mut Vec<u8>,
        greeting_codec: &GreetingCodec,
        response_codec: &ResponseCodec,
    ) -> CurrentMessage {
        match self {
            QueuedMessage::Greeting { greeting } => {
                let encoded = greeting_codec.encode(&greeting);
                push_encoded_to_buffer(write_buffer, encoded);
                CurrentMessage::Greeting { greeting }
            }
            QueuedMessage::Response { handle, response } => {
                let encoded = response_codec.encode(&response);
                push_encoded_to_buffer(write_buffer, encoded);
                CurrentMessage::Response { handle, response }
            }
        }
    }
}

fn push_encoded_to_buffer(write_buffer: &mut Vec<u8>, encoded: Encoded) {
    for fragment in encoded {
        let data = match fragment {
            Fragment::Line { data } => data,
            // Note: The server doesn't need to wait before sending a literal.
            //       Thus, non-sync literals doesn't make sense here.
            //       This is currently an issue in imap-codec,
            //       see https://github.com/duesee/imap-codec/issues/332
            Fragment::Literal { data, .. } => data,
        };
        write_buffer.extend(data);
    }
}

/// Message that is currently being sent.
enum CurrentMessage {
    Greeting {
        greeting: Greeting<'static>,
    },
    Response {
        handle: Option<ResponseHandle>,
        response: Response<'static>,
    },
}

/// Message was sent.
pub enum ServerSendEvent {
    Greeting {
        greeting: Greeting<'static>,
    },
    Response {
        handle: Option<ResponseHandle>,
        response: Response<'static>,
    },
}
