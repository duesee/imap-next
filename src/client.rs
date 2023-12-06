use std::{borrow::Cow, fmt::Debug};

use bytes::BytesMut;
use imap_codec::{
    decode::{GreetingDecodeError, ResponseDecodeError},
    imap_types::{
        auth::{AuthMechanism, AuthenticateData},
        command::{Command, CommandBody},
        core::Tag,
        response::{
            CommandContinuationRequest, Data, Greeting, Response, Status, StatusBody, StatusKind,
            Tagged,
        },
        secret::Secret,
    },
    CommandCodec, GreetingCodec, ResponseCodec,
};
use thiserror::Error;

use crate::{
    handle::{Handle, HandleGenerator, HandleGeneratorGenerator, RawHandle},
    receive::{ReceiveEvent, ReceiveState},
    send::{SendCommandKind, SendCommandState},
    stream::{AnyStream, StreamError},
};

static HANDLE_GENERATOR_GENERATOR: HandleGeneratorGenerator<ClientFlowCommandHandle> =
    HandleGeneratorGenerator::new();

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientFlowOptions {
    pub crlf_relaxed: bool,
}

impl Default for ClientFlowOptions {
    fn default() -> Self {
        Self {
            // Lean towards usability
            crlf_relaxed: true,
        }
    }
}

#[derive(Debug)]
pub struct ClientFlow {
    stream: AnyStream,

    handle_generator: HandleGenerator<ClientFlowCommandHandle>,
    send_command_state: SendCommandState<ClientFlowCommandHandle>,
    receive_response_state: ReceiveState<ResponseCodec>,
}

impl ClientFlow {
    pub async fn receive_greeting(
        mut stream: AnyStream,
        options: ClientFlowOptions,
    ) -> Result<(Self, Greeting<'static>), ClientFlowError> {
        // Receive greeting.
        let mut receive_greeting_state = ReceiveState::new(
            GreetingCodec::default(),
            options.crlf_relaxed,
            BytesMut::new(),
        );

        let greeting = match receive_greeting_state.progress(&mut stream).await? {
            ReceiveEvent::DecodingSuccess(greeting) => {
                receive_greeting_state.finish_message();
                greeting
            }
            ReceiveEvent::DecodingFailure(
                GreetingDecodeError::Failed | GreetingDecodeError::Incomplete,
            ) => {
                let discarded_bytes = receive_greeting_state.discard_message();
                return Err(ClientFlowError::MalformedMessage { discarded_bytes });
            }
            ReceiveEvent::ExpectedCrlfGotLf => {
                let discarded_bytes = receive_greeting_state.discard_message();
                return Err(ClientFlowError::ExpectedCrlfGotLf { discarded_bytes });
            }
        };

        // Create state to send commands ...
        let send_command_state = SendCommandState::new(CommandCodec::default(), BytesMut::new());

        // ..., and state to receive responses.
        let receive_response_state = ReceiveState::new(
            ResponseCodec::new(),
            options.crlf_relaxed,
            receive_greeting_state.finish(),
        );

        let client_flow = Self {
            stream,
            handle_generator: HANDLE_GENERATOR_GENERATOR.generate(),
            send_command_state,
            receive_response_state,
        };

        Ok((client_flow, greeting))
    }

    /// Enqueues the [`Command`] for being sent to the client.
    ///
    /// The [`Command`] is not sent immediately but during one of the next calls of
    /// [`ClientFlow::progress`]. All [`Command`]s are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_command(&mut self, command: Command<'static>) -> ClientFlowCommandHandle {
        if let CommandBody::Authenticate { .. } = command.body {
            // TODO: Remember that we send the AUTHENTICATE.
            // The server will request additional data (if required).
            todo!()
        }

        let handle = self.handle_generator.generate();
        self.send_command_state.enqueue(handle, command);
        handle
    }

    pub async fn progress(&mut self) -> Result<ClientFlowEvent, ClientFlowError> {
        // The client must do two things:
        // - Sending commands to the server.
        // - Receiving responses from the server.
        //
        // There are two ways to accomplish that:
        // - Doing both in parallel.
        // - Doing both consecutively.
        //
        // Doing both in parallel is more complicated because both operations share the same
        // state and the borrow checker prevents the naive approach. We would need to introduce
        // interior mutability.
        //
        // Doing both consecutively is easier. But in which order? Receiving responses will block
        // indefinitely because we never know when the server is sending the next response.
        // Sending commands will be completed in the foreseeable future because for practical
        // purposes we can assume that the number of commands is finite and the stream will be
        // able to transfer all bytes soon.
        //
        // Therefore we prefer the second approach and begin with sending the commands.
        loop {
            if let Some(event) = self.progress_command().await? {
                return Ok(event);
            }

            if let Some(event) = self.progress_response().await? {
                return Ok(event);
            }
        }
    }

    async fn progress_command(&mut self) -> Result<Option<ClientFlowEvent>, ClientFlowError> {
        match self.send_command_state.progress(&mut self.stream).await? {
            Some((handle, command)) => Ok(Some(ClientFlowEvent::CommandSent { handle, command })),
            None => Ok(None),
        }
    }

    async fn progress_response(&mut self) -> Result<Option<ClientFlowEvent>, ClientFlowError> {
        let event = loop {
            let response = match self
                .receive_response_state
                .progress(&mut self.stream)
                .await?
            {
                ReceiveEvent::DecodingSuccess(response) => {
                    self.receive_response_state.finish_message();
                    response
                }
                ReceiveEvent::DecodingFailure(ResponseDecodeError::LiteralFound { length }) => {
                    // The client must accept the literal in any case.
                    self.receive_response_state.start_literal(length);
                    continue;
                }
                ReceiveEvent::DecodingFailure(
                    ResponseDecodeError::Failed | ResponseDecodeError::Incomplete,
                ) => {
                    let discarded_bytes = self.receive_response_state.discard_message();
                    return Err(ClientFlowError::MalformedMessage { discarded_bytes });
                }
                ReceiveEvent::ExpectedCrlfGotLf => {
                    let discarded_bytes = self.receive_response_state.discard_message();
                    return Err(ClientFlowError::ExpectedCrlfGotLf { discarded_bytes });
                }
            };

            match response {
                Response::Status(status) => {
                    if let Some((handle, command)) = self.maybe_abort_command(&status) {
                        break Some(ClientFlowEvent::CommandRejected {
                            handle,
                            command,
                            status,
                        });
                    } else if let Some(result) = self.maybe_finish_authenticate(&status) {
                        match result {
                            FinishAuthenticateResult::Accepted {
                                handle,
                                tag,
                                mechanism,
                                initial_response,
                                authenticate_data,
                            } => {
                                break Some(ClientFlowEvent::AuthenticationAccepted {
                                    handle,
                                    tag,
                                    mechanism,
                                    initial_response,
                                    authenticate_data,
                                    status,
                                });
                            }
                            FinishAuthenticateResult::Rejected {
                                handle,
                                tag,
                                mechanism,
                                initial_response,
                                authenticate_data,
                            } => {
                                break Some(ClientFlowEvent::AuthenticationRejected {
                                    handle,
                                    tag,
                                    mechanism,
                                    initial_response,
                                    authenticate_data,
                                    status,
                                });
                            }
                        }
                    } else {
                        break Some(ClientFlowEvent::StatusReceived { status });
                    }
                }
                Response::Data(data) => break Some(ClientFlowEvent::DataReceived { data }),
                Response::CommandContinuationRequest(continuation) => {
                    if self.send_command_state.continue_command() {
                        break None;
                    } else if let Some(handle) = self.send_command_state.continue_authenticate() {
                        break Some(ClientFlowEvent::AuthenticationContinue {
                            handle,
                            continuation,
                        });
                    } else {
                        break Some(ClientFlowEvent::ContinuationReceived { continuation });
                    }
                }
            }
        };

        Ok(event)
    }

    fn maybe_abort_command(
        &mut self,
        status: &Status,
    ) -> Option<(ClientFlowCommandHandle, Command<'static>)> {
        let command = self.send_command_state.command_in_progress()?;

        match status {
            Status::Tagged(Tagged {
                tag,
                body: StatusBody { kind, .. },
                ..
            }) if *kind == StatusKind::Bad && tag == &command.tag => {
                self.send_command_state.abort_command()
            }
            _ => None,
        }
    }

    fn maybe_finish_authenticate(&mut self, status: &Status) -> Option<FinishAuthenticateResult> {
        todo!()
    }

    pub fn authenticate_continue(
        &mut self,
        authenticate_data: AuthenticateData,
    ) -> Result<ClientFlowCommandHandle, AuthenticateData> {
        self.send_command_state
            .continue_authenticate_with_data(authenticate_data)
    }
}

enum FinishAuthenticateResult {
    Accepted {
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
        mechanism: AuthMechanism<'static>,
        initial_response: Option<Secret<Cow<'static, [u8]>>>,
        authenticate_data: Vec<AuthenticateData>,
    },
    // TODO: handle, command, ...
    Rejected {
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
        mechanism: AuthMechanism<'static>,
        initial_response: Option<Secret<Cow<'static, [u8]>>>,
        authenticate_data: Vec<AuthenticateData>,
    },
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
    /// Server is requesting (more) authentication data.
    /// TODO: This could also be encoded in ContinuationReceived (see below).
    AuthenticationContinue {
        /// Handle to the enqueued [`Command`].
        handle: ClientFlowCommandHandle,
        continuation: CommandContinuationRequest<'static>,
    },
    // TODO: handle, command, ...
    AuthenticationAccepted {
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
        mechanism: AuthMechanism<'static>,
        initial_response: Option<Secret<Cow<'static, [u8]>>>,
        authenticate_data: Vec<AuthenticateData>,
        status: Status<'static>,
    },
    // TODO: handle, command, ...
    AuthenticationRejected {
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
        mechanism: AuthMechanism<'static>,
        initial_response: Option<Secret<Cow<'static, [u8]>>>,
        authenticate_data: Vec<AuthenticateData>,
        status: Status<'static>,
    },
    /// Enqueued [`Command`] successfully sent.
    CommandSent {
        /// Handle to the enqueued [`Command`].
        handle: ClientFlowCommandHandle,
        /// Formerly enqueued [`Command`].
        command: Command<'static>,
        // TODO:
        // more_data: ...
        // status: Option<Status<'static>>,
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
        /// (e.g. [`Code::Alert`](imap_codec::imap_types::response::Code::Alert)).
        status: Status<'static>,
    },
    /// Server [`Data`] received.
    DataReceived { data: Data<'static> },
    /// Server [`Status`] received.
    StatusReceived { status: Status<'static> },
    /// Server [`CommandContinuationRequest`] response received.
    ///
    /// Note: The received continuation was not part of [`ClientFlow`] literal handling.
    /// It is either ...
    /// * an acknowledgement to send authentication data,
    /// * an acknowledgement to proceed with IDLE,
    /// * or an unsolicited continuation (in which case processing is deferred to the user).
    // TODO remove?
    ContinuationReceived {
        // TODO: Add a context instead?
        // context: ContinuationContext,
        continuation: CommandContinuationRequest<'static>,
    },
}

// TODO
/*
pub enum ContinuationContext {
    Unknown,
    Authenticate {
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
    },
    Idle {
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
    },
}
*/

#[derive(Debug, Error)]
pub enum ClientFlowError {
    #[error(transparent)]
    Stream(#[from] StreamError),
    #[error("Expected `\\r\\n`, got `\\n`")]
    ExpectedCrlfGotLf { discarded_bytes: Box<[u8]> },
    #[error("Received malformed message")]
    MalformedMessage { discarded_bytes: Box<[u8]> },
}
