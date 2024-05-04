use std::fmt::{Debug, Formatter};

use imap_codec::{
    decode::{AuthenticateDataDecodeError, CommandDecodeError, IdleDoneDecodeError},
    AuthenticateDataCodec, CommandCodec, GreetingCodec, IdleDoneCodec, ResponseCodec,
};
use imap_types::{
    auth::AuthenticateData,
    command::{Command, CommandBody},
    core::{LiteralMode, Tag, Text},
    extensions::idle::IdleDone,
    response::{CommandContinuationRequest, Data, Greeting, Response, Status},
    secret::Secret,
};
use thiserror::Error;

use crate::{
    handle::{Handle, HandleGenerator, HandleGeneratorGenerator, RawHandle},
    receive::{ReceiveError, ReceiveEvent, ReceiveState},
    send_response::{SendResponseEvent, SendResponseState},
    types::CommandAuthenticate,
    Flow, FlowInterrupt,
};

static HANDLE_GENERATOR_GENERATOR: HandleGeneratorGenerator<ServerFlowResponseHandle> =
    HandleGeneratorGenerator::new();

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ServerFlowOptions {
    pub crlf_relaxed: bool,
    /// Max literal size accepted by server.
    ///
    /// Bigger literals are rejected by the server.
    ///
    /// Currently we don't distinguish between general literals and the literal used in the
    /// APPEND command. However, this might change in the future. Note that
    /// `max_literal_size < max_command_size` must hold.
    pub max_literal_size: u32,
    /// Max command size that can be parsed by the server.
    ///
    /// Bigger commands raise an error.
    pub max_command_size: u32,
    pub literal_accept_text: Text<'static>,
    pub literal_reject_text: Text<'static>,
}

impl Default for ServerFlowOptions {
    fn default() -> Self {
        Self {
            // Lean towards conformity
            crlf_relaxed: false,
            // 25 MiB is a common maximum email size (Oct. 2023).
            max_literal_size: 25 * 1024 * 1024,
            // Must be bigger than `max_literal_size`.
            // 64 KiB is used by Dovecot.
            max_command_size: (25 * 1024 * 1024) + (64 * 1024),
            // Short unmeaning text
            literal_accept_text: Text::unvalidated("..."),
            // Short unmeaning text
            literal_reject_text: Text::unvalidated("..."),
        }
    }
}

pub struct ServerFlow {
    options: ServerFlowOptions,
    handle_generator: HandleGenerator<ServerFlowResponseHandle>,
    send_response_state: SendResponseState,
    receive_state: ServerReceiveState,
}

impl Debug for ServerFlow {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.debug_struct("ServerFlow")
            .field("options", &self.options)
            .field("handle_generator", &self.handle_generator)
            .finish_non_exhaustive()
    }
}

impl Flow for ServerFlow {
    type Event = ServerFlowEvent;
    type Error = ServerFlowError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.enqueue_input(bytes)
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        self.progress()
    }
}

impl ServerFlow {
    pub fn new(options: ServerFlowOptions, greeting: Greeting<'static>) -> Self {
        let mut send_response_state =
            SendResponseState::new(GreetingCodec::default(), ResponseCodec::default());

        send_response_state.enqueue_greeting(greeting);

        let receive_state = ServerReceiveState::Command(ReceiveState::new(
            CommandCodec::default(),
            options.crlf_relaxed,
            Some(options.max_command_size),
        ));

        Self {
            options,
            handle_generator: HANDLE_GENERATOR_GENERATOR.generate(),
            send_response_state,
            receive_state,
        }
    }

    pub fn enqueue_input(&mut self, bytes: &[u8]) {
        match &mut self.receive_state {
            ServerReceiveState::Command(state) => state.enqueue_input(bytes),
            ServerReceiveState::AuthenticateData(state) => state.enqueue_input(bytes),
            ServerReceiveState::IdleAccept(state) => state.enqueue_input(bytes),
            ServerReceiveState::IdleDone(state) => state.enqueue_input(bytes),
            ServerReceiveState::Dummy => unreachable!(),
        }
    }

    /// Enqueues the [`Data`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`ServerFlow::progress`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_data(&mut self, data: Data<'static>) -> ServerFlowResponseHandle {
        let handle = self.handle_generator.generate();
        self.send_response_state
            .enqueue_response(Some(handle), Response::Data(data));
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
            .enqueue_response(Some(handle), Response::Status(status));
        handle
    }

    /// Enqueues the [`CommandContinuationRequest`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`ServerFlow::progress`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_continuation_request(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> ServerFlowResponseHandle {
        let handle = self.handle_generator.generate();
        self.send_response_state.enqueue_response(
            Some(handle),
            Response::CommandContinuationRequest(continuation_request),
        );
        handle
    }

    pub fn progress(&mut self) -> Result<ServerFlowEvent, FlowInterrupt<ServerFlowError>> {
        loop {
            if let Some(event) = self.progress_send()? {
                return Ok(event);
            }

            if let Some(event) = self.progress_receive()? {
                return Ok(event);
            }
        }
    }

    fn progress_send(&mut self) -> Result<Option<ServerFlowEvent>, FlowInterrupt<ServerFlowError>> {
        match self.send_response_state.progress() {
            Ok(Some(SendResponseEvent::Greeting { greeting })) => {
                // The initial greeting was sucessfully sent, inform the caller
                Ok(Some(ServerFlowEvent::GreetingSent { greeting }))
            }
            Ok(Some(SendResponseEvent::Response {
                handle: Some(handle),
                response,
            })) => {
                // A response was sucessfully sent, inform the caller
                Ok(Some(ServerFlowEvent::ResponseSent { handle, response }))
            }
            Ok(Some(SendResponseEvent::Response { handle: None, .. })) => {
                // An internally created response was sent, don't inform the caller
                Ok(None)
            }
            Ok(_) => {
                // No progress yet
                Ok(None)
            }
            Err(FlowInterrupt::Io(io)) => Err(FlowInterrupt::Io(io)),
            Err(FlowInterrupt::Error(_)) => unreachable!(),
        }
    }

    fn progress_receive(
        &mut self,
    ) -> Result<Option<ServerFlowEvent>, FlowInterrupt<ServerFlowError>> {
        match &mut self.receive_state {
            ServerReceiveState::Command(state) => {
                match state.progress() {
                    Ok(ReceiveEvent::DecodingSuccess(command)) => {
                        state.finish_message();

                        match command.body {
                            CommandBody::Authenticate {
                                mechanism,
                                initial_response,
                            } => {
                                self.receive_state
                                    .change_state(NextExpectedMessage::AuthenticateData);

                                Ok(Some(ServerFlowEvent::CommandAuthenticateReceived {
                                    command_authenticate: CommandAuthenticate {
                                        tag: command.tag,
                                        mechanism,
                                        initial_response,
                                    },
                                }))
                            }
                            CommandBody::Idle => {
                                self.receive_state
                                    .change_state(NextExpectedMessage::IdleAccept);

                                Ok(Some(ServerFlowEvent::IdleCommandReceived {
                                    tag: command.tag,
                                }))
                            }
                            body => Ok(Some(ServerFlowEvent::CommandReceived {
                                command: Command {
                                    tag: command.tag,
                                    body,
                                },
                            })),
                        }
                    }
                    Err(FlowInterrupt::Io(io)) => Err(FlowInterrupt::Io(io)),
                    Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                        CommandDecodeError::LiteralFound { tag, length, mode },
                    ))) => {
                        if length > self.options.max_literal_size {
                            match mode {
                                LiteralMode::Sync => {
                                    // Inform the client that the literal was rejected.

                                    // Unwrap: This should never fail because the text is not Base64.
                                    let status = Status::bad(
                                        Some(tag),
                                        None,
                                        self.options.literal_reject_text.clone(),
                                    )
                                    .unwrap();
                                    self.send_response_state
                                        .enqueue_response(None, Response::Status(status));

                                    let discarded_bytes = state.discard_message();

                                    Err(FlowInterrupt::Error(ServerFlowError::LiteralTooLong {
                                        discarded_bytes: Secret::new(discarded_bytes),
                                    }))
                                }
                                LiteralMode::NonSync => {
                                    // TODO: We can't (reliably) make the client stop sending data.
                                    //       Some actions that come to mind:
                                    //       * terminate the connection
                                    //       * act as a "discard server", i.e., consume the full
                                    //         literal w/o saving it, and answering with `BAD`
                                    //       * ...
                                    //
                                    //       The LITERAL+ RFC has some recommendations.
                                    let discarded_bytes = state.discard_message();

                                    Err(FlowInterrupt::Error(ServerFlowError::LiteralTooLong {
                                        discarded_bytes: Secret::new(discarded_bytes),
                                    }))
                                }
                            }
                        } else {
                            state.start_literal(length);

                            match mode {
                                LiteralMode::Sync => {
                                    // Inform the client that the literal was accepted.

                                    // Unwrap: This should never fail because the text is not Base64.
                                    let cont = CommandContinuationRequest::basic(
                                        None,
                                        self.options.literal_accept_text.clone(),
                                    )
                                    .unwrap();
                                    self.send_response_state.enqueue_response(
                                        None,
                                        Response::CommandContinuationRequest(cont),
                                    );
                                }
                                LiteralMode::NonSync => {
                                    // We don't need to inform the client because non-sync literals
                                    // are automatically accepted.
                                }
                            }

                            Ok(None)
                        }
                    }
                    Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                        CommandDecodeError::Failed | CommandDecodeError::Incomplete,
                    ))) => {
                        let discarded_bytes = state.discard_message();
                        Err(FlowInterrupt::Error(ServerFlowError::MalformedMessage {
                            discarded_bytes: Secret::new(discarded_bytes),
                        }))
                    }
                    Err(FlowInterrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                        let discarded_bytes = state.discard_message();
                        Err(FlowInterrupt::Error(ServerFlowError::ExpectedCrlfGotLf {
                            discarded_bytes: Secret::new(discarded_bytes),
                        }))
                    }
                    Err(FlowInterrupt::Error(ReceiveError::MessageTooLong)) => {
                        let discarded_bytes = state.discard_message();
                        Err(FlowInterrupt::Error(ServerFlowError::CommandTooLong {
                            discarded_bytes: Secret::new(discarded_bytes),
                        }))
                    }
                }
            }
            ServerReceiveState::AuthenticateData(state) => match state.progress() {
                Ok(ReceiveEvent::DecodingSuccess(authenticate_data)) => {
                    state.finish_message();
                    Ok(Some(ServerFlowEvent::AuthenticateDataReceived {
                        authenticate_data,
                    }))
                }
                Err(FlowInterrupt::Io(io)) => Err(FlowInterrupt::Io(io)),
                Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                    AuthenticateDataDecodeError::Failed | AuthenticateDataDecodeError::Incomplete,
                ))) => {
                    let discarded_bytes = state.discard_message();
                    Err(FlowInterrupt::Error(ServerFlowError::MalformedMessage {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(FlowInterrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                    let discarded_bytes = state.discard_message();
                    Err(FlowInterrupt::Error(ServerFlowError::ExpectedCrlfGotLf {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(FlowInterrupt::Error(ReceiveError::MessageTooLong)) => {
                    let discarded_bytes = state.discard_message();
                    Err(FlowInterrupt::Error(ServerFlowError::CommandTooLong {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
            },
            ServerReceiveState::IdleAccept(_) => {
                // We don't expect any message until the server flow user calls
                // `idle_accept` or `idle_reject`.
                // TODO: It's strange to return NeedMoreInput here, but it works for now.
                Err(FlowInterrupt::Io(crate::FlowIo::NeedMoreInput))
            }
            ServerReceiveState::IdleDone(state) => match state.progress() {
                Ok(ReceiveEvent::DecodingSuccess(IdleDone)) => {
                    state.finish_message();

                    self.receive_state
                        .change_state(NextExpectedMessage::Command);

                    Ok(Some(ServerFlowEvent::IdleDoneReceived))
                }
                Err(FlowInterrupt::Io(io)) => Err(FlowInterrupt::Io(io)),
                Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                    IdleDoneDecodeError::Failed | IdleDoneDecodeError::Incomplete,
                ))) => {
                    let discarded_bytes = state.discard_message();
                    Err(FlowInterrupt::Error(ServerFlowError::MalformedMessage {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(FlowInterrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                    let discarded_bytes = state.discard_message();
                    Err(FlowInterrupt::Error(ServerFlowError::ExpectedCrlfGotLf {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(FlowInterrupt::Error(ReceiveError::MessageTooLong)) => {
                    let discarded_bytes = state.discard_message();
                    Err(FlowInterrupt::Error(ServerFlowError::CommandTooLong {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
            },
            ServerReceiveState::Dummy => {
                unreachable!()
            }
        }
    }

    pub fn authenticate_continue(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> Result<ServerFlowResponseHandle, CommandContinuationRequest<'static>> {
        if let ServerReceiveState::AuthenticateData { .. } = self.receive_state {
            let handle = self.enqueue_continuation_request(continuation_request);
            Ok(handle)
        } else {
            Err(continuation_request)
        }
    }

    pub fn authenticate_finish(
        &mut self,
        status: Status<'static>,
    ) -> Result<ServerFlowResponseHandle, Status<'static>> {
        if let ServerReceiveState::AuthenticateData(_) = &mut self.receive_state {
            let handle = self.enqueue_status(status);

            self.receive_state
                .change_state(NextExpectedMessage::Command);

            Ok(handle)
        } else {
            Err(status)
        }
    }

    pub fn idle_accept(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> Result<ServerFlowResponseHandle, CommandContinuationRequest<'static>> {
        if let ServerReceiveState::IdleAccept(_) = &mut self.receive_state {
            let handle = self.enqueue_continuation_request(continuation_request);

            self.receive_state
                .change_state(NextExpectedMessage::IdleDone);

            Ok(handle)
        } else {
            Err(continuation_request)
        }
    }

    pub fn idle_reject(
        &mut self,
        status: Status<'static>,
    ) -> Result<ServerFlowResponseHandle, Status<'static>> {
        if let ServerReceiveState::IdleAccept(_) = &mut self.receive_state {
            let handle = self.enqueue_status(status);

            self.receive_state
                .change_state(NextExpectedMessage::Command);

            Ok(handle)
        } else {
            Err(status)
        }
    }
}

#[derive(Clone, Copy)]
enum NextExpectedMessage {
    Command,
    AuthenticateData,
    IdleAccept,
    IdleDone,
}

enum ServerReceiveState {
    Command(ReceiveState<CommandCodec>),
    AuthenticateData(ReceiveState<AuthenticateDataCodec>),
    IdleAccept(ReceiveState<NoCodec>),
    IdleDone(ReceiveState<IdleDoneCodec>),
    // This state is set only temporarily during `ServerReceiveState::change_state`
    Dummy,
}

impl ServerReceiveState {
    fn change_state(&mut self, next_expected_message: NextExpectedMessage) {
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
    /// Initial [`Greeting] was sent successfully.
    GreetingSent {
        greeting: Greeting<'static>,
    },
    /// Enqueued [`Response`] was sent successfully.
    ResponseSent {
        /// Handle of the formerly enqueued [`Response`].
        handle: ServerFlowResponseHandle,
        /// Formerly enqueued [`Response`] that was now sent.
        response: Response<'static>,
    },
    /// Command received.
    CommandReceived {
        command: Command<'static>,
    },
    /// Command AUTHENTICATE received.
    ///
    /// Note: The server MUST call [`ServerFlow::authenticate_continue`] (if it needs more data for
    /// authentication) or [`ServerFlow::authenticate_finish`] (if there already is enough data for
    /// authentication) next. "Enough data" is determined by the used SASL mechanism, if there was
    /// an initial response (SASL-IR), etc.
    CommandAuthenticateReceived {
        command_authenticate: CommandAuthenticate,
    },
    /// Continuation to AUTHENTICATE received.
    ///
    /// Note: The server MUST call [`ServerFlow::authenticate_continue`] (if it needs more data for
    /// authentication) or [`ServerFlow::authenticate_finish`] (if there already is enough data for
    /// authentication) next. "Enough data" is determined by the used SASL mechanism, if there was
    /// an initial response (SASL-IR), etc.
    ///
    /// Note, too: The client may abort the authentication by using [`AuthenticateData::Cancel`].
    /// Make sure to honor the client's request to not end up in an infinite loop. It's up to the
    /// server to end the authentication flow.
    AuthenticateDataReceived {
        authenticate_data: AuthenticateData<'static>,
    },
    IdleCommandReceived {
        tag: Tag<'static>,
    },
    IdleDoneReceived,
}

#[derive(Debug, Error)]
pub enum ServerFlowError {
    #[error("Expected `\\r\\n`, got `\\n`")]
    ExpectedCrlfGotLf { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Received malformed message")]
    MalformedMessage { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Literal was rejected because it was too long")]
    LiteralTooLong { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Command was rejected because it was too long")]
    CommandTooLong { discarded_bytes: Secret<Box<[u8]>> },
}

/// A dummy codec we use for technical reasons when we don't want to receive anything at all.
struct NoCodec;
