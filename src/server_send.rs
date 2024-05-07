use std::{collections::VecDeque, convert::Infallible};

use imap_codec::{
    encode::{Encoder, Fragment},
    GreetingCodec, ResponseCodec,
};
use imap_types::response::{Greeting, Response};

use crate::{server::ServerFlowResponseHandle, FlowInterrupt, FlowIo};

pub struct SendResponseState {
    greeting_codec: GreetingCodec,
    response_codec: ResponseCodec,
    // FIFO queue for responses that should be sent next.
    queued_responses: VecDeque<QueuedResponse>,
    // The response that is currently being sent.
    current_response: Option<CurrentResponse>,
}

impl SendResponseState {
    pub fn new(greeting_codec: GreetingCodec, response_codec: ResponseCodec) -> Self {
        Self {
            greeting_codec,
            response_codec,
            queued_responses: VecDeque::new(),
            current_response: None,
        }
    }

    pub fn enqueue_greeting(&mut self, greeting: Greeting<'static>) {
        self.queued_responses.push_back(QueuedResponse {
            handle: None,
            response: ResponseMessage::Greeting(greeting),
        });
    }

    pub fn enqueue_response(
        &mut self,
        handle: Option<ServerFlowResponseHandle>,
        response: Response<'static>,
    ) {
        self.queued_responses.push_back(QueuedResponse {
            handle,
            response: ResponseMessage::Response(response),
        });
    }

    pub fn progress(&mut self) -> Result<Option<SendResponseEvent>, FlowInterrupt<Infallible>> {
        match self.current_response.take() {
            Some(current_response) => {
                // Continue the response that was interrupted.
                let handle = current_response.handle;
                let event = match current_response.response {
                    ResponseMessage::Greeting(greeting) => SendResponseEvent::Greeting { greeting },
                    ResponseMessage::Response(response) => {
                        SendResponseEvent::Response { handle, response }
                    }
                };
                Ok(Some(event))
            }
            None => {
                let Some(queued_response) = self.queued_responses.pop_front() else {
                    // There is currently no response that needs to be sent
                    return Ok(None);
                };

                // Creates a buffer for writing the current response
                let mut write_buffer = Vec::new();

                // Push the bytes of the response to the buffer
                let current_response = queued_response.push_to_buffer(
                    &mut write_buffer,
                    &self.greeting_codec,
                    &self.response_codec,
                );

                self.current_response = Some(current_response);

                // Interrupt the flow for sendng all bytes of current response
                Err(FlowInterrupt::Io(FlowIo::Output(write_buffer)))
            }
        }
    }
}

enum ResponseMessage {
    Greeting(Greeting<'static>),
    Response(Response<'static>),
}

/// A response that is queued but not sent yet.
struct QueuedResponse {
    handle: Option<ServerFlowResponseHandle>,
    response: ResponseMessage,
}

impl QueuedResponse {
    fn push_to_buffer(
        self,
        write_buffer: &mut Vec<u8>,
        greeting_codec: &GreetingCodec,
        response_codec: &ResponseCodec,
    ) -> CurrentResponse {
        let fragments = match &self.response {
            ResponseMessage::Greeting(greeting) => greeting_codec.encode(greeting),
            ResponseMessage::Response(response) => response_codec.encode(response),
        };
        for fragment in fragments {
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

        CurrentResponse {
            handle: self.handle,
            response: self.response,
        }
    }
}

/// A response that is currently being sent.
struct CurrentResponse {
    handle: Option<ServerFlowResponseHandle>,
    response: ResponseMessage,
}

/// A response was sent.
pub enum SendResponseEvent {
    Greeting {
        greeting: Greeting<'static>,
    },
    Response {
        handle: Option<ServerFlowResponseHandle>,
        response: Response<'static>,
    },
}
