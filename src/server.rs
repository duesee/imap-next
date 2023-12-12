use std::{fmt::Debug, mem};

use bytes::BytesMut;
use imap_codec::{
    decode::{AuthenticateDataDecodeError, CommandDecodeError},
    imap_types::{
        auth::AuthenticateData,
        command::{Command, CommandBody},
        core::Text,
        response::{CommandContinuationRequest, Data, Greeting, Response, Status},
    },
    AuthenticateDataCodec, CommandCodec, GreetingCodec, ResponseCodec,
};
use thiserror::Error;

use crate::{
    handle::{Handle, HandleGenerator, HandleGeneratorGenerator, RawHandle},
    receive::{ReceiveEvent, ReceiveState},
    send::SendResponseState,
    stream::{AnyStream, StreamError},
    types::CommandAuthenticate,
};

static HANDLE_GENERATOR_GENERATOR: HandleGeneratorGenerator<ServerFlowResponseHandle> =
    HandleGeneratorGenerator::new();

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
    options: ServerFlowOptions,

    handle_generator: HandleGenerator<ServerFlowResponseHandle>,
    send_response_state: SendResponseState<ResponseCodec, Option<ServerFlowResponseHandle>>,
    receive_command_state: ServerReceiveState,
}

#[derive(Debug)]
enum ServerReceiveState {
    Command(ReceiveState<CommandCodec>),
    AuthenticateData(ReceiveState<AuthenticateDataCodec>),
    Dummy,
}

impl From<ReceiveState<CommandCodec>> for ServerReceiveState {
    fn from(state: ReceiveState<CommandCodec>) -> Self {
        Self::Command(state)
    }
}

impl From<ReceiveState<AuthenticateDataCodec>> for ServerReceiveState {
    fn from(state: ReceiveState<AuthenticateDataCodec>) -> Self {
        Self::AuthenticateData(state)
    }
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
            options,
            handle_generator: HANDLE_GENERATOR_GENERATOR.generate(),
            send_response_state,
            receive_command_state: receive_command_state.into(),
        };

        Ok((server_flow, greeting))
    }

    /// Enqueues the [`Data`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`ServerFlow::progress`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_data(&mut self, data: Data<'static>) -> ServerFlowResponseHandle {
        let handle = self.handle_generator.generate();
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
        let handle = self.handle_generator.generate();
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
        let handle = self.handle_generator.generate();
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
            if let Some(event) = self.progress_send().await? {
                return Ok(event);
            }

            if let Some(event) = self.progress_receive().await? {
                return Ok(event);
            }
        }
    }

    async fn progress_send(&mut self) -> Result<Option<ServerFlowEvent>, ServerFlowError> {
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

    async fn progress_receive(&mut self) -> Result<Option<ServerFlowEvent>, ServerFlowError> {
        let receive_command_state =
            mem::replace(&mut self.receive_command_state, ServerReceiveState::Dummy);

        let (next_receive_command_state, result) = match receive_command_state {
            ServerReceiveState::Command(state) => self.progress_receive_command(state).await,
            ServerReceiveState::AuthenticateData(state) => {
                self.progress_receive_authenticate_data(state).await
            }
            ServerReceiveState::Dummy => unreachable!(),
        };

        self.receive_command_state = next_receive_command_state;

        result
    }

    async fn progress_receive_command(
        &mut self,
        mut state: ReceiveState<CommandCodec>,
    ) -> (
        ServerReceiveState,
        Result<Option<ServerFlowEvent>, ServerFlowError>,
    ) {
        let event = match state.progress(&mut self.stream).await {
            Ok(event) => event,
            Err(error) => return (state.into(), Err(error.into())),
        };

        match event {
            ReceiveEvent::DecodingSuccess(command) => {
                state.finish_message();

                match command.body {
                    CommandBody::Authenticate {
                        mechanism,
                        initial_response,
                    } => {
                        let next_state = ReceiveState::new(
                            AuthenticateDataCodec::new(),
                            self.options.crlf_relaxed,
                            state.finish(),
                        );

                        (
                            next_state.into(),
                            Ok(Some(ServerFlowEvent::CommandAuthenticateReceived {
                                command_authenticate: CommandAuthenticate {
                                    tag: command.tag,
                                    mechanism,
                                    initial_response,
                                },
                            })),
                        )
                    }
                    body => (
                        state.into(),
                        Ok(Some(ServerFlowEvent::CommandReceived {
                            command: Command {
                                tag: command.tag,
                                body,
                            },
                        })),
                    ),
                }
            }
            ReceiveEvent::DecodingFailure(CommandDecodeError::LiteralFound {
                tag,
                length,
                mode: _mode,
            }) => {
                if length > self.options.max_literal_size {
                    let discarded_bytes = state.discard_message();

                    // Inform the client that the literal was rejected.
                    // This should never fail because the text is not Base64.
                    let status =
                        Status::no(Some(tag), None, self.options.literal_reject_text.clone())
                            .unwrap();
                    self.send_response_state
                        .enqueue(None, Response::Status(status));

                    (
                        state.into(),
                        Err(ServerFlowError::LiteralTooLong { discarded_bytes }),
                    )
                } else {
                    state.start_literal(length);

                    // Inform the client that the literal was accepted.
                    // This should never fail because the text is not Base64.
                    let cont = CommandContinuationRequest::basic(
                        None,
                        self.options.literal_accept_text.clone(),
                    )
                    .unwrap();
                    self.send_response_state
                        .enqueue(None, Response::CommandContinuationRequest(cont));

                    (state.into(), Ok(None))
                }
            }
            ReceiveEvent::DecodingFailure(
                CommandDecodeError::Failed | CommandDecodeError::Incomplete,
            ) => {
                let discarded_bytes = state.discard_message();
                (
                    state.into(),
                    Err(ServerFlowError::MalformedMessage { discarded_bytes }),
                )
            }
            ReceiveEvent::ExpectedCrlfGotLf => {
                let discarded_bytes = state.discard_message();
                (
                    state.into(),
                    Err(ServerFlowError::ExpectedCrlfGotLf { discarded_bytes }),
                )
            }
        }
    }

    async fn progress_receive_authenticate_data(
        &mut self,
        mut state: ReceiveState<AuthenticateDataCodec>,
    ) -> (
        ServerReceiveState,
        Result<Option<ServerFlowEvent>, ServerFlowError>,
    ) {
        let event = match state.progress(&mut self.stream).await {
            Ok(event) => event,
            Err(error) => return (state.into(), Err(error.into())),
        };

        match event {
            ReceiveEvent::DecodingSuccess(authenticate_data) => {
                state.finish_message();
                (
                    state.into(),
                    Ok(Some(ServerFlowEvent::AuthenticateDataReceived {
                        authenticate_data,
                    })),
                )
            }
            ReceiveEvent::DecodingFailure(
                AuthenticateDataDecodeError::Failed | AuthenticateDataDecodeError::Incomplete,
            ) => {
                let discarded_bytes = state.discard_message();
                (
                    state.into(),
                    Err(ServerFlowError::MalformedMessage { discarded_bytes }),
                )
            }
            ReceiveEvent::ExpectedCrlfGotLf => {
                let discarded_bytes = state.discard_message();
                (
                    state.into(),
                    Err(ServerFlowError::ExpectedCrlfGotLf { discarded_bytes }),
                )
            }
        }
    }

    pub fn authenticate_continue(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Result<ServerFlowResponseHandle, ServerFlowError> {
        if let ServerReceiveState::AuthenticateData { .. } = self.receive_command_state {
            let handle = self.enqueue_continuation(continuation);
            Ok(handle)
        } else {
            Err(ServerFlowError::BadState)
        }
    }

    pub fn authenticate_finish(
        &mut self,
        status: Status<'static>,
    ) -> Result<ServerFlowResponseHandle, ServerFlowError> {
        let receive_command_state =
            mem::replace(&mut self.receive_command_state, ServerReceiveState::Dummy);

        let (result, next_state) =
            if let ServerReceiveState::AuthenticateData(state) = receive_command_state {
                let handle = self.enqueue_status(status);

                let state = ReceiveState::new(
                    CommandCodec::new(),
                    self.options.crlf_relaxed,
                    state.finish(),
                );

                (Ok(handle), state.into())
            } else {
                (Err(ServerFlowError::BadState), receive_command_state)
            };

        self.receive_command_state = next_state;

        result
    }
}

/// A handle for an enqueued [`Response`].
///
/// This handle can be used to track the sending progress. After a [`Response`] was enqueued via
/// [`ServerFlow::enqueue_data`] or [`ServerFlow::enqueue_status`] it is in the process of being
/// sent until [`ServerFlow::progress`] returns a [`ServerFlowEvent::ResponseSent`] with the
/// corresponding handle.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ServerFlowResponseHandle(RawHandle);

impl Handle for ServerFlowResponseHandle {
    fn from_raw(raw_handle: RawHandle) -> Self {
        Self(raw_handle)
    }
}

// Implement a short debug representation that hides the underlying raw handle
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
    /// Command received.
    CommandReceived { command: Command<'static> },
    /// Command AUTHENTICATE received.
    CommandAuthenticateReceived {
        command_authenticate: CommandAuthenticate,
    },
    /// Continuation to AUTHENTICATE received.
    ///
    /// Note: This can either mean `Continue` or `Cancel` depending on `authenticate_data`.
    AuthenticateDataReceived { authenticate_data: AuthenticateData },
    /// Enqueued [`Response`] was sent successfully.
    ResponseSent {
        /// Handle of the formerly enqueued [`Response`].
        handle: ServerFlowResponseHandle,
        /// Formerly enqueued [`Response`] that was now sent.
        response: Response<'static>,
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
    #[error("bad state")]
    BadState, // TODO Damian: Hustenjakob doesn't like this
}
