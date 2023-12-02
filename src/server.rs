use std::fmt::Debug;

use bytes::BytesMut;
use imap_codec::{
    decode::CommandDecodeError,
    imap_types::{
        command::Command,
        core::Text,
        response::{CommandContinuationRequest, Data, Greeting, Response, Status},
    },
    CommandCodec, GreetingCodec, ResponseCodec,
};
use thiserror::Error;

use crate::{
    handle::{Handle, HandleGenerator, HandleGeneratorGenerator},
    receive::{ReceiveEvent, ReceiveState},
    send::SendResponseState,
    stream::{AnyStream, StreamError},
};

static HANDLE_GENERATOR_GENERATOR: HandleGeneratorGenerator = HandleGeneratorGenerator::new();

#[derive(Debug, Clone, PartialEq)]
pub struct ServerFlowOptions {
    pub crlf_relaxed: bool,
    pub max_literal_size: u32,
    pub literal_accept_text: Text<'static>,
    pub literal_reject_text: Text<'static>,
}

impl Default for ServerFlowOptions {
    fn default() -> Self {
        Self {
            // Lean towards usability
            crlf_relaxed: true,
            // 25 MiB is a common maximum email size (Oct. 2023)
            max_literal_size: 25 * 1024 * 1024,
            // Short unmeaning text
            literal_accept_text: Text::unvalidated("..."),
            // Short unmeaning text
            literal_reject_text: Text::unvalidated("..."),
        }
    }
}

#[derive(Debug)]
pub struct ServerFlow {
    stream: AnyStream,
    max_literal_size: u32,

    handle_generator: HandleGenerator,
    send_response_state: SendResponseState<ResponseCodec, Option<ServerFlowResponseHandle>>,
    receive_command_state: ReceiveState<CommandCodec>,

    literal_accept_text: Text<'static>,
    literal_reject_text: Text<'static>,
}

impl ServerFlow {
    pub async fn send_greeting(
        mut stream: AnyStream,
        options: ServerFlowOptions,
        greeting: Greeting<'static>,
    ) -> Result<(Self, Greeting<'static>), ServerFlowError> {
        // Send greeting
        let write_buffer = BytesMut::new();
        let mut send_greeting_state =
            SendResponseState::new(GreetingCodec::default(), write_buffer);
        send_greeting_state.enqueue((), greeting);
        let greeting = loop {
            if let Some(((), greeting)) = send_greeting_state.progress(&mut stream).await? {
                break greeting;
            }
        };

        // Successfully sent greeting, construct instance
        let write_buffer = send_greeting_state.finish();
        let send_response_state = SendResponseState::new(ResponseCodec::default(), write_buffer);
        let read_buffer = BytesMut::new();
        let receive_command_state =
            ReceiveState::new(CommandCodec::default(), options.crlf_relaxed, read_buffer);
        let server_flow = Self {
            stream,
            max_literal_size: options.max_literal_size,
            handle_generator: HANDLE_GENERATOR_GENERATOR.generate(),
            send_response_state,
            receive_command_state,
            literal_accept_text: options.literal_accept_text,
            literal_reject_text: options.literal_reject_text,
        };

        Ok((server_flow, greeting))
    }

    /// Enqueues the [`Data`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`ServerFlow::progress`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_data(&mut self, data: Data<'static>) -> ServerFlowResponseHandle {
        let handle = ServerFlowResponseHandle(self.handle_generator.generate());
        self.send_response_state
            .enqueue(Some(handle), Response::Data(data));
        handle
    }

    /// Enqueues the [`Status`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`ServerFlow::progress`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_status(&mut self, status: Status<'static>) -> ServerFlowResponseHandle {
        let handle = ServerFlowResponseHandle(self.handle_generator.generate());
        self.send_response_state
            .enqueue(Some(handle), Response::Status(status));
        handle
    }

    /// Enqueues the [`CommandContinuationRequest`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`ServerFlow::progress`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_continuation(
        &mut self,
        cotinuation: CommandContinuationRequest<'static>,
    ) -> ServerFlowResponseHandle {
        let handle = ServerFlowResponseHandle(self.handle_generator.generate());
        self.send_response_state.enqueue(
            Some(handle),
            Response::CommandContinuationRequest(cotinuation),
        );
        handle
    }

    pub async fn progress(&mut self) -> Result<ServerFlowEvent, ServerFlowError> {
        // The server must do two things:
        // - Sending responses to the client.
        // - Receiving commands from the client.
        //
        // There are two ways to accomplish that:
        // - Doing both in parallel.
        // - Doing both consecutively.
        //
        // Doing both in parallel is more complicated because both operations share the same
        // state and the borrow checker prevents the naive approach. We would need to introduce
        // interior mutability.
        //
        // Doing both consecutively is easier. But in which order? Receiving commands will block
        // indefinitely because we never know when the client is sending the next response.
        // Sending responses will be completed in the foreseeable future because for practical
        // purposes we can assume that the number of responses is finite and the stream will be
        // able to transfer all bytes soon.
        //
        // Therefore we prefer the second approach and begin with sending the responses.
        loop {
            if let Some(event) = self.progress_response().await? {
                return Ok(event);
            }

            if let Some(event) = self.progress_command().await? {
                return Ok(event);
            }
        }
    }

    async fn progress_response(&mut self) -> Result<Option<ServerFlowEvent>, ServerFlowError> {
        match self.send_response_state.progress(&mut self.stream).await? {
            Some((Some(handle), response)) => {
                // A response was sucessfully sent, inform the caller
                Ok(Some(ServerFlowEvent::ResponseSent { handle, response }))
            }
            Some((None, _)) => {
                // An internally created response was sent, don't inform the caller
                Ok(None)
            }
            _ => {
                // No progress yet
                Ok(None)
            }
        }
    }

    async fn progress_command(&mut self) -> Result<Option<ServerFlowEvent>, ServerFlowError> {
        match self
            .receive_command_state
            .progress(&mut self.stream)
            .await?
        {
            ReceiveEvent::DecodingSuccess(command) => {
                self.receive_command_state.finish_message();
                Ok(Some(ServerFlowEvent::CommandReceived { command }))
            }
            ReceiveEvent::DecodingFailure(CommandDecodeError::LiteralFound {
                tag,
                length,
                mode: _mode,
            }) => {
                if length > self.max_literal_size {
                    let discarded_bytes = self.receive_command_state.discard_message();

                    // Inform the client that the literal was rejected.
                    // This should never fail because the text is not Base64.
                    let status =
                        Status::no(Some(tag), None, self.literal_reject_text.clone()).unwrap();
                    self.send_response_state
                        .enqueue(None, Response::Status(status));

                    Err(ServerFlowError::LiteralTooLong { discarded_bytes })
                } else {
                    self.receive_command_state.start_literal(length);

                    // Inform the client that the literal was accepted.
                    // This should never fail because the text is not Base64.
                    let cont =
                        CommandContinuationRequest::basic(None, self.literal_accept_text.clone())
                            .unwrap();
                    self.send_response_state
                        .enqueue(None, Response::CommandContinuationRequest(cont));

                    Ok(None)
                }
            }
            ReceiveEvent::DecodingFailure(
                CommandDecodeError::Failed | CommandDecodeError::Incomplete,
            ) => {
                let discarded_bytes = self.receive_command_state.discard_message();
                Err(ServerFlowError::MalformedMessage { discarded_bytes })
            }
            ReceiveEvent::ExpectedCrlfGotLf => {
                let discarded_bytes = self.receive_command_state.discard_message();
                Err(ServerFlowError::ExpectedCrlfGotLf { discarded_bytes })
            }
        }
    }
}

/// A handle for an enqueued [`Response`].
///
/// This handle can be used to track the sending progress. After a [`Response`] was enqueued via
/// [`ServerFlow::enqueue_data`] or [`ServerFlow::enqueue_status`] it is in the process of being
/// sent until [`ServerFlow::progress`] returns a [`ServerFlowEvent::ResponseSent`] with the
/// corresponding handle.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ServerFlowResponseHandle(Handle);

impl Debug for ServerFlowResponseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ServerFlowResponseHandle")
            .field(&self.0.generator_id())
            .field(&self.0.handle_id())
            .finish()
    }
}

#[derive(Debug)]
pub enum ServerFlowEvent {
    /// The enqueued [`Response`] was sent successfully.
    ResponseSent {
        /// The handle of the enqueued [`Response`].
        handle: ServerFlowResponseHandle,
        /// Formerly enqueued [`Response`] that was now sent.
        response: Response<'static>,
    },
    CommandReceived {
        command: Command<'static>,
    },
}

#[derive(Debug, Error)]
pub enum ServerFlowError {
    #[error(transparent)]
    Stream(#[from] StreamError),
    #[error("Expected `\\r\\n`, got `\\n`")]
    ExpectedCrlfGotLf { discarded_bytes: Box<[u8]> },
    #[error("Received malformed message")]
    MalformedMessage { discarded_bytes: Box<[u8]> },
    #[error("Literal was rejected because it was too long")]
    LiteralTooLong { discarded_bytes: Box<[u8]> },
}
