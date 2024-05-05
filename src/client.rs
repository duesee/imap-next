use std::fmt::{Debug, Formatter};

use imap_codec::{
    decode::{GreetingDecodeError, ResponseDecodeError},
    AuthenticateDataCodec, CommandCodec, GreetingCodec, IdleDoneCodec, ResponseCodec,
};
use imap_types::{
    auth::AuthenticateData,
    command::Command,
    response::{CommandContinuationRequest, Data, Greeting, Response, Status},
    secret::Secret,
};
use thiserror::Error;

use crate::{
    handle::{Handle, HandleGenerator, HandleGeneratorGenerator, RawHandle},
    receive::{ReceiveError, ReceiveEvent, ReceiveState},
    send_command::{SendCommandEvent, SendCommandState, SendCommandTermination},
    types::CommandAuthenticate,
    Flow, FlowInterrupt,
};

static HANDLE_GENERATOR_GENERATOR: HandleGeneratorGenerator<ClientFlowCommandHandle> =
    HandleGeneratorGenerator::new();

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ClientFlowOptions {
    pub crlf_relaxed: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for ClientFlowOptions {
    fn default() -> Self {
        Self {
            // Lean towards conformity
            crlf_relaxed: false,
        }
    }
}

pub struct ClientFlow {
    options: ClientFlowOptions,
    handle_generator: HandleGenerator<ClientFlowCommandHandle>,
    send_command_state: SendCommandState,
    receive_state: ClientReceiveState,
}

impl Debug for ClientFlow {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.debug_struct("ClientFlow")
            .field("handle_generator", &self.handle_generator)
            .finish_non_exhaustive()
    }
}

impl Flow for ClientFlow {
    type Event = ClientFlowEvent;
    type Error = ClientFlowError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.enqueue_input(bytes)
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        self.progress()
    }
}

impl ClientFlow {
    pub fn use_response_codec(&mut self) {
        self.receive_state = ClientReceiveState::Response(ReceiveState::new(
            ResponseCodec::default(),
            self.options.crlf_relaxed,
            None,
        ))
    }

    pub fn new(options: ClientFlowOptions) -> Self {
        let send_command_state = SendCommandState::new(
            CommandCodec::default(),
            AuthenticateDataCodec::default(),
            IdleDoneCodec::default(),
        );

        let receive_state = ClientReceiveState::Greeting(ReceiveState::new(
            GreetingCodec::default(),
            options.crlf_relaxed,
            None,
        ));

        Self {
            options,
            handle_generator: HANDLE_GENERATOR_GENERATOR.generate(),
            send_command_state,
            receive_state,
        }
    }

    pub fn enqueue_input(&mut self, bytes: &[u8]) {
        match &mut self.receive_state {
            ClientReceiveState::Greeting(state) => state.enqueue_input(bytes),
            ClientReceiveState::Response(state) => state.enqueue_input(bytes),
            ClientReceiveState::Dummy => unreachable!(),
        }
    }

    /// Enqueues the [`Command`] for being sent to the client.
    ///
    /// The [`Command`] is not sent immediately but during one of the next calls of
    /// [`ClientFlow::progress`]. All [`Command`]s are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_command(&mut self, command: Command<'static>) -> ClientFlowCommandHandle {
        let handle = self.handle_generator.generate();
        self.send_command_state.enqueue(handle, command);
        handle
    }

    pub fn progress(&mut self) -> Result<ClientFlowEvent, FlowInterrupt<ClientFlowError>> {
        loop {
            if let Some(event) = self.progress_send()? {
                return Ok(event);
            }

            if let Some(event) = self.progress_receive()? {
                return Ok(event);
            }
        }
    }

    fn progress_send(&mut self) -> Result<Option<ClientFlowEvent>, FlowInterrupt<ClientFlowError>> {
        // Abort if we didn't received the greeting yet
        if let ClientReceiveState::Greeting(_) = &self.receive_state {
            return Ok(None);
        }

        match self.send_command_state.progress() {
            Ok(Some(SendCommandEvent::Command { handle, command })) => {
                Ok(Some(ClientFlowEvent::CommandSent { handle, command }))
            }
            Ok(Some(SendCommandEvent::CommandAuthenticate { handle })) => {
                Ok(Some(ClientFlowEvent::AuthenticateStarted { handle }))
            }
            Ok(Some(SendCommandEvent::CommandIdle { handle })) => {
                Ok(Some(ClientFlowEvent::IdleCommandSent { handle }))
            }
            Ok(Some(SendCommandEvent::IdleDone { handle })) => {
                Ok(Some(ClientFlowEvent::IdleDoneSent { handle }))
            }
            Ok(None) => Ok(None),
            Err(FlowInterrupt::Io(io)) => Err(FlowInterrupt::Io(io)),
            Err(FlowInterrupt::Error(_)) => unreachable!(),
        }
    }

    fn progress_receive(
        &mut self,
    ) -> Result<Option<ClientFlowEvent>, FlowInterrupt<ClientFlowError>> {
        let event = loop {
            match &mut self.receive_state {
                ClientReceiveState::Greeting(state) => {
                    match state.progress() {
                        Ok(ReceiveEvent::DecodingSuccess(greeting)) => {
                            state.finish_message();
                            self.receive_state.change_state();
                            break Some(ClientFlowEvent::GreetingReceived { greeting });
                        }
                        Err(FlowInterrupt::Io(io)) => return Err(FlowInterrupt::Io(io)),
                        Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                            GreetingDecodeError::Failed | GreetingDecodeError::Incomplete,
                        ))) => {
                            let discarded_bytes = state.discard_message();
                            return Err(FlowInterrupt::Error(ClientFlowError::MalformedMessage {
                                discarded_bytes: Secret::new(discarded_bytes),
                            }));
                        }
                        Err(FlowInterrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                            let discarded_bytes = state.discard_message();
                            return Err(FlowInterrupt::Error(ClientFlowError::ExpectedCrlfGotLf {
                                discarded_bytes: Secret::new(discarded_bytes),
                            }));
                        }
                        Err(FlowInterrupt::Error(ReceiveError::MessageTooLong)) => {
                            // Unreachable because message limit is not set
                            unreachable!()
                        }
                    }
                }
                ClientReceiveState::Response(state) => {
                    let response = match state.progress() {
                        Ok(ReceiveEvent::DecodingSuccess(response)) => {
                            state.finish_message();
                            response
                        }
                        Err(FlowInterrupt::Io(io)) => return Err(FlowInterrupt::Io(io)),
                        Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                            ResponseDecodeError::LiteralFound { length },
                        ))) => {
                            // The client must accept the literal in any case.
                            state.start_literal(length);
                            continue;
                        }
                        Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                            ResponseDecodeError::Failed | ResponseDecodeError::Incomplete,
                        ))) => {
                            let discarded_bytes = state.discard_message();
                            return Err(FlowInterrupt::Error(ClientFlowError::MalformedMessage {
                                discarded_bytes: Secret::new(discarded_bytes),
                            }));
                        }
                        Err(FlowInterrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                            let discarded_bytes = state.discard_message();
                            return Err(FlowInterrupt::Error(ClientFlowError::ExpectedCrlfGotLf {
                                discarded_bytes: Secret::new(discarded_bytes),
                            }));
                        }
                        Err(FlowInterrupt::Error(ReceiveError::MessageTooLong)) => {
                            // Unreachable because message limit is not set
                            unreachable!()
                        }
                    };

                    match response {
                        Response::Status(status) => {
                            let event = if let Some(finish_result) =
                                self.send_command_state.maybe_remove(&status)
                            {
                                match finish_result {
                                    SendCommandTermination::LiteralRejected { handle, command } => {
                                        ClientFlowEvent::CommandRejected {
                                            handle,
                                            command,
                                            status,
                                        }
                                    }
                                    SendCommandTermination::AuthenticateAccepted {
                                        handle,
                                        command_authenticate,
                                    }
                                    | SendCommandTermination::AuthenticateRejected {
                                        handle,
                                        command_authenticate,
                                    } => ClientFlowEvent::AuthenticateStatusReceived {
                                        handle,
                                        command_authenticate,
                                        status,
                                    },
                                    SendCommandTermination::IdleRejected { handle } => {
                                        ClientFlowEvent::IdleRejected { handle, status }
                                    }
                                }
                            } else {
                                ClientFlowEvent::StatusReceived { status }
                            };

                            break Some(event);
                        }
                        Response::Data(data) => break Some(ClientFlowEvent::DataReceived { data }),
                        Response::CommandContinuationRequest(continuation_request) => {
                            if self.send_command_state.literal_continue() {
                                // We received a continuation request that was necessary for sending a command.
                                // So we abort receiving responses for now and continue with sending commands.
                                break None;
                            } else if let Some(handle) =
                                self.send_command_state.authenticate_continue()
                            {
                                break Some(
                                    ClientFlowEvent::AuthenticateContinuationRequestReceived {
                                        handle,
                                        continuation_request,
                                    },
                                );
                            } else if let Some(handle) = self.send_command_state.idle_continue() {
                                break Some(ClientFlowEvent::IdleAccepted {
                                    handle,
                                    continuation_request,
                                });
                            } else {
                                break Some(ClientFlowEvent::ContinuationRequestReceived {
                                    continuation_request,
                                });
                            }
                        }
                    }
                }
                ClientReceiveState::Dummy => {
                    unreachable!()
                }
            }
        };

        Ok(event)
    }

    pub fn set_authenticate_data(
        &mut self,
        authenticate_data: AuthenticateData<'static>,
    ) -> Result<ClientFlowCommandHandle, AuthenticateData<'static>> {
        self.send_command_state
            .set_authenticate_data(authenticate_data)
    }

    pub fn set_idle_done(&mut self) -> Option<ClientFlowCommandHandle> {
        self.send_command_state.set_idle_done()
    }
}

enum ClientReceiveState {
    Greeting(ReceiveState<GreetingCodec>),
    Response(ReceiveState<ResponseCodec>),
    // This state is set only temporarily during `ClientReceiveState::change_state`
    Dummy,
}

impl ClientReceiveState {
    fn change_state(&mut self) {
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

/// A handle for an enqueued [`Command`].
///
/// This handle can be used to track the sending progress. After a [`Command`] was enqueued via
/// [`ClientFlow::enqueue_command`] it is in the process of being sent until
/// [`ClientFlow::progress`] returns a [`ClientFlowEvent::CommandSent`] or
/// [`ClientFlowEvent::CommandRejected`] with the corresponding handle.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ClientFlowCommandHandle(RawHandle);

impl Handle for ClientFlowCommandHandle {
    fn from_raw(handle: RawHandle) -> Self {
        Self(handle)
    }
}

// Implement a short debug representation that hides the underlying raw handle
impl Debug for ClientFlowCommandHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ClientFlowCommandHandle")
            .field(&self.0.generator_id())
            .field(&self.0.handle_id())
            .finish()
    }
}

#[derive(Debug)]
pub enum ClientFlowEvent {
    /// Server [`Greeting`] received.
    GreetingReceived {
        greeting: Greeting<'static>,
    },
    /// Enqueued [`Command`] successfully sent.
    CommandSent {
        /// Handle to the enqueued [`Command`].
        handle: ClientFlowCommandHandle,
        /// Formerly enqueued [`Command`].
        command: Command<'static>,
    },
    /// Enqueued [`Command`] rejected.
    ///
    /// Note: Emitted when the server rejected a command literal with `BAD`.
    CommandRejected {
        /// Handle to the enqueued [`Command`].
        handle: ClientFlowCommandHandle,
        /// Formerly enqueued [`Command`].
        command: Command<'static>,
        /// [`Status`] sent by the server in order to reject the [`Command`].
        ///
        /// Note: [`ClientFlow`] already handled this [`Status`] but it might still have
        /// useful information that could be logged or displayed to the user
        /// (e.g. [`Code::Alert`](imap_types::response::Code::Alert)).
        status: Status<'static>,
    },
    AuthenticateStarted {
        handle: ClientFlowCommandHandle,
    },
    /// Server is requesting (more) authentication data.
    ///
    /// The client MUST call [`ClientFlow::set_authenticate_data`] next.
    ///
    /// Note: The client can also progress the authentication by sending [`AuthenticateData::Cancel`].
    /// However, it's up to the server to abort the authentication flow by sending a tagged status response.
    AuthenticateContinuationRequestReceived {
        /// Handle to the enqueued [`Command`].
        handle: ClientFlowCommandHandle,
        continuation_request: CommandContinuationRequest<'static>,
    },
    AuthenticateStatusReceived {
        handle: ClientFlowCommandHandle,
        command_authenticate: CommandAuthenticate,
        status: Status<'static>,
    },
    IdleCommandSent {
        handle: ClientFlowCommandHandle,
    },
    IdleAccepted {
        handle: ClientFlowCommandHandle,
        continuation_request: CommandContinuationRequest<'static>,
    },
    IdleRejected {
        handle: ClientFlowCommandHandle,
        status: Status<'static>,
    },
    IdleDoneSent {
        handle: ClientFlowCommandHandle,
    },
    /// Server [`Data`] received.
    DataReceived {
        data: Data<'static>,
    },
    /// Server [`Status`] received.
    StatusReceived {
        status: Status<'static>,
    },
    /// Server [`CommandContinuationRequest`] response received.
    ///
    /// Note: The received continuation request was not part of [`ClientFlow`] handling.
    ContinuationRequestReceived {
        continuation_request: CommandContinuationRequest<'static>,
    },
}

#[derive(Debug, Error)]
pub enum ClientFlowError {
    #[error("Expected `\\r\\n`, got `\\n`")]
    ExpectedCrlfGotLf { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Received malformed message")]
    MalformedMessage { discarded_bytes: Secret<Box<[u8]>> },
}
