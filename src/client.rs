use bytes::BytesMut;
use imap_codec::{
    decode::{GreetingDecodeError, ResponseDecodeError},
    encode::Encoder,
    imap_types::{
        auth::AuthenticateData,
        command::Command,
        response::{
            CommandContinuationRequest, Data, Greeting, Response, Status, StatusBody, StatusKind,
            Tagged,
        },
    },
    AuthenticateDataCodec, CommandCodec, GreetingCodec, ResponseCodec,
};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use crate::{
    receive::{ReceiveEvent, ReceiveState},
    send::SendCommandState,
    stream::AnyStream,
};

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

    handle_generator: ClientFlowCommandHandleGenerator,
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
            handle_generator: ClientFlowCommandHandleGenerator::default(),
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
        let handle = self.handle_generator.generate();
        self.send_command_state.enqueue(handle, command);
        handle
    }

    // TODO
    pub async fn enqueue_authenticate_data(&mut self, data: AuthenticateData) {
        self.stream
            .0
            .write_all(&AuthenticateDataCodec::new().encode(&data).dump())
            .await
            .unwrap();
    }

    pub async fn progress(&mut self) -> Result<ClientFlowEvent, ClientFlowError> {
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
                    } else {
                        break Some(ClientFlowEvent::StatusReceived { status });
                    }
                }
                Response::Data(data) => break Some(ClientFlowEvent::DataReceived { data }),
                Response::CommandContinuationRequest(continuation) => {
                    if self.send_command_state.continue_command() {
                        break None;
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
}

/// A handle for an enqueued [`Command`].
///
/// This handle can be used to track the sending progress. After a [`Command`] was enqueued via
/// [`ClientFlow::enqueue_command`] it is in the process of being sent until
/// [`ClientFlow::progress`] returns a [`ClientFlowEvent::CommandSent`] or
/// [`ClientFlowEvent::CommandRejected`] with the corresponding handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ClientFlowCommandHandle(u64);

#[derive(Debug)]
pub enum ClientFlowEvent {
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
    ContinuationReceived {
        continuation: CommandContinuationRequest<'static>,
    },
}

#[derive(Debug, Error)]
pub enum ClientFlowError {
    #[error(transparent)]
    Io(#[from] tokio::io::Error),
    #[error("Expected `\\r\\n`, got `\\n`")]
    ExpectedCrlfGotLf { discarded_bytes: Box<[u8]> },
    #[error("Received malformed message")]
    MalformedMessage { discarded_bytes: Box<[u8]> },
}

#[derive(Debug, Default)]
struct ClientFlowCommandHandleGenerator {
    counter: u64,
}

impl ClientFlowCommandHandleGenerator {
    fn generate(&mut self) -> ClientFlowCommandHandle {
        let handle = ClientFlowCommandHandle(self.counter);
        self.counter += self.counter.wrapping_add(1);
        handle
    }
}
