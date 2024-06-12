use std::fmt::{Debug, Formatter};

use bounded_static::ToBoundedStatic;
use imap_codec::{
    decode::{AuthenticateDataDecodeError, CommandDecodeError, IdleDoneDecodeError},
    CommandCodec, GreetingCodec, ResponseCodec,
};
use imap_types::{
    auth::AuthenticateData,
    command::{Command, CommandBody},
    core::{LiteralMode, Tag, Text},
    extensions::idle::IdleDone,
    response::{
        CommandContinuationRequest, CommandContinuationRequestBasic, Data, Greeting, Response,
        Status,
    },
    secret::Secret,
};
use thiserror::Error;

use crate::{
    handle::{Handle, HandleGenerator, HandleGeneratorGenerator, RawHandle},
    receive::{ReceiveError, ReceiveEvent, ReceiveState},
    server_receive::{NextExpectedMessage, ServerReceiveState},
    server_send::{ServerSendEvent, ServerSendState},
    types::CommandAuthenticate,
    Interrupt, State,
};

static HANDLE_GENERATOR_GENERATOR: HandleGeneratorGenerator<ResponseHandle> =
    HandleGeneratorGenerator::new();

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Options {
    pub crlf_relaxed: bool,
    /// Max literal size accepted by server.
    ///
    /// Bigger literals are rejected by the server.
    ///
    /// Currently, we don't distinguish between general literals and the literal used in the
    /// APPEND command. However, this might change in the future. Note that
    /// `max_literal_size < max_command_size` must hold.
    pub max_literal_size: u32,
    /// Max command size that can be parsed by the server.
    ///
    /// Bigger commands raise an error.
    pub max_command_size: u32,
    literal_accept_ccr: CommandContinuationRequest<'static>,
    literal_reject_ccr: CommandContinuationRequest<'static>,
}

impl Default for Options {
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
            literal_accept_ccr: CommandContinuationRequest::basic(None, Text::unvalidated("..."))
                .unwrap(),
            // Short unmeaning text
            literal_reject_ccr: CommandContinuationRequest::basic(None, Text::unvalidated("..."))
                .unwrap(),
        }
    }
}

impl Options {
    pub fn literal_accept_text(&self) -> &Text {
        match self.literal_accept_ccr {
            CommandContinuationRequest::Basic(ref basic) => basic.text(),
            CommandContinuationRequest::Base64(_) => unreachable!(),
        }
    }

    pub fn set_literal_accept_text(&mut self, text: String) -> Result<(), String> {
        // imap-codec doesn't return `text` on error. Thus, we first check with &str as a
        // workaround ...
        if CommandContinuationRequestBasic::new(None, text.as_str()).is_ok() {
            // ... and can use `unwrap` later.
            self.literal_accept_ccr = CommandContinuationRequest::basic(None, text).unwrap();
            Ok(())
        } else {
            Err(text)
        }
    }

    pub fn literal_reject_text(&self) -> &Text {
        match self.literal_reject_ccr {
            CommandContinuationRequest::Basic(ref basic) => basic.text(),
            CommandContinuationRequest::Base64(_) => unreachable!(),
        }
    }

    pub fn set_literal_reject_text(&mut self, text: String) -> Result<(), String> {
        // imap-codec doesn't return `text` on error. Thus, we first check with &str as a
        // workaround ...
        if CommandContinuationRequestBasic::new(None, text.as_str()).is_ok() {
            // ... and can use `unwrap` later.
            self.literal_reject_ccr = CommandContinuationRequest::basic(None, text).unwrap();
            Ok(())
        } else {
            Err(text)
        }
    }
}

pub struct Server {
    options: Options,
    handle_generator: HandleGenerator<ResponseHandle>,
    send_state: ServerSendState,
    receive_state: ServerReceiveState,
}

impl Server {
    pub fn new(options: Options, greeting: Greeting<'static>) -> Self {
        let mut send_state =
            ServerSendState::new(GreetingCodec::default(), ResponseCodec::default());

        send_state.enqueue_greeting(greeting);

        let receive_state = ServerReceiveState::Command(ReceiveState::new(
            CommandCodec::default(),
            options.crlf_relaxed,
            Some(options.max_command_size),
        ));

        Self {
            options,
            handle_generator: HANDLE_GENERATOR_GENERATOR.generate(),
            send_state,
            receive_state,
        }
    }

    /// Enqueues the [`Data`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`Server::next`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_data(&mut self, data: Data<'static>) -> ResponseHandle {
        let handle = self.handle_generator.generate();
        self.send_state
            .enqueue_response(Some(handle), Response::Data(data));
        handle
    }

    /// Enqueues the [`Status`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`Server::next`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_status(&mut self, status: Status<'static>) -> ResponseHandle {
        let handle = self.handle_generator.generate();
        self.send_state
            .enqueue_response(Some(handle), Response::Status(status));
        handle
    }

    /// Enqueues the [`CommandContinuationRequest`] response for being sent to the client.
    ///
    /// The response is not sent immediately but during one of the next calls of
    /// [`Server::next`]. All responses are sent in the same order they have been
    /// enqueued.
    pub fn enqueue_continuation_request(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> ResponseHandle {
        let handle = self.handle_generator.generate();
        self.send_state.enqueue_response(
            Some(handle),
            Response::CommandContinuationRequest(continuation_request),
        );
        handle
    }

    fn progress_send(&mut self) -> Result<Option<Event>, Interrupt<Error>> {
        match self.send_state.next() {
            Ok(Some(ServerSendEvent::Greeting { greeting })) => {
                // The initial greeting was sucessfully sent, inform the caller
                Ok(Some(Event::GreetingSent { greeting }))
            }
            Ok(Some(ServerSendEvent::Response {
                handle: Some(handle),
                response,
            })) => {
                // A response was sucessfully sent, inform the caller
                Ok(Some(Event::ResponseSent { handle, response }))
            }
            Ok(Some(ServerSendEvent::Response { handle: None, .. })) => {
                // An internally created response was sent, don't inform the caller
                Ok(None)
            }
            Ok(_) => {
                // No progress yet
                Ok(None)
            }
            Err(Interrupt::Io(io)) => Err(Interrupt::Io(io)),
            Err(Interrupt::Error(_)) => unreachable!(),
        }
    }

    fn progress_receive(&mut self) -> Result<Option<Event>, Interrupt<Error>> {
        match &mut self.receive_state {
            ServerReceiveState::Command(state) => {
                match state.next() {
                    Ok(ReceiveEvent::DecodingSuccess(command)) => {
                        state.finish_message();

                        match command.body {
                            CommandBody::Authenticate {
                                mechanism,
                                initial_response,
                            } => {
                                self.receive_state
                                    .change_state(NextExpectedMessage::AuthenticateData);

                                Ok(Some(Event::CommandAuthenticateReceived {
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

                                Ok(Some(Event::IdleCommandReceived { tag: command.tag }))
                            }
                            body => Ok(Some(Event::CommandReceived {
                                command: Command {
                                    tag: command.tag,
                                    body,
                                },
                            })),
                        }
                    }
                    Err(Interrupt::Io(io)) => Err(Interrupt::Io(io)),
                    Err(Interrupt::Error(ReceiveError::DecodingFailure(
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
                                        self.options.literal_reject_text().to_static(),
                                    )
                                    .unwrap();
                                    self.send_state
                                        .enqueue_response(None, Response::Status(status));

                                    let discarded_bytes = state.discard_message();

                                    Err(Interrupt::Error(Error::LiteralTooLong {
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

                                    Err(Interrupt::Error(Error::LiteralTooLong {
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
                                        self.options.literal_accept_text().to_static(),
                                    )
                                    .unwrap();
                                    self.send_state.enqueue_response(
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
                    Err(Interrupt::Error(ReceiveError::DecodingFailure(
                        CommandDecodeError::Failed | CommandDecodeError::Incomplete,
                    ))) => {
                        let discarded_bytes = state.discard_message();
                        Err(Interrupt::Error(Error::MalformedMessage {
                            discarded_bytes: Secret::new(discarded_bytes),
                        }))
                    }
                    Err(Interrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                        let discarded_bytes = state.discard_message();
                        Err(Interrupt::Error(Error::ExpectedCrlfGotLf {
                            discarded_bytes: Secret::new(discarded_bytes),
                        }))
                    }
                    Err(Interrupt::Error(ReceiveError::MessageTooLong)) => {
                        let discarded_bytes = state.discard_message();
                        Err(Interrupt::Error(Error::CommandTooLong {
                            discarded_bytes: Secret::new(discarded_bytes),
                        }))
                    }
                }
            }
            ServerReceiveState::AuthenticateData(state) => match state.next() {
                Ok(ReceiveEvent::DecodingSuccess(authenticate_data)) => {
                    state.finish_message();
                    Ok(Some(Event::AuthenticateDataReceived { authenticate_data }))
                }
                Err(Interrupt::Io(io)) => Err(Interrupt::Io(io)),
                Err(Interrupt::Error(ReceiveError::DecodingFailure(
                    AuthenticateDataDecodeError::Failed | AuthenticateDataDecodeError::Incomplete,
                ))) => {
                    let discarded_bytes = state.discard_message();
                    Err(Interrupt::Error(Error::MalformedMessage {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(Interrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                    let discarded_bytes = state.discard_message();
                    Err(Interrupt::Error(Error::ExpectedCrlfGotLf {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(Interrupt::Error(ReceiveError::MessageTooLong)) => {
                    let discarded_bytes = state.discard_message();
                    Err(Interrupt::Error(Error::CommandTooLong {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
            },
            ServerReceiveState::IdleAccept(_) => {
                // We don't expect any message until the server user calls
                // `idle_accept` or `idle_reject`.
                // TODO: It's strange to return NeedMoreInput here, but it works for now.
                Err(Interrupt::Io(crate::Io::NeedMoreInput))
            }
            ServerReceiveState::IdleDone(state) => match state.next() {
                Ok(ReceiveEvent::DecodingSuccess(IdleDone)) => {
                    state.finish_message();

                    self.receive_state
                        .change_state(NextExpectedMessage::Command);

                    Ok(Some(Event::IdleDoneReceived))
                }
                Err(Interrupt::Io(io)) => Err(Interrupt::Io(io)),
                Err(Interrupt::Error(ReceiveError::DecodingFailure(
                    IdleDoneDecodeError::Failed | IdleDoneDecodeError::Incomplete,
                ))) => {
                    let discarded_bytes = state.discard_message();
                    Err(Interrupt::Error(Error::MalformedMessage {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(Interrupt::Error(ReceiveError::ExpectedCrlfGotLf)) => {
                    let discarded_bytes = state.discard_message();
                    Err(Interrupt::Error(Error::ExpectedCrlfGotLf {
                        discarded_bytes: Secret::new(discarded_bytes),
                    }))
                }
                Err(Interrupt::Error(ReceiveError::MessageTooLong)) => {
                    let discarded_bytes = state.discard_message();
                    Err(Interrupt::Error(Error::CommandTooLong {
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
    ) -> Result<ResponseHandle, CommandContinuationRequest<'static>> {
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
    ) -> Result<ResponseHandle, Status<'static>> {
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
    ) -> Result<ResponseHandle, CommandContinuationRequest<'static>> {
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
    ) -> Result<ResponseHandle, Status<'static>> {
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

impl Debug for Server {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("options", &self.options)
            .field("handle_generator", &self.handle_generator)
            .finish_non_exhaustive()
    }
}

impl State for Server {
    type Event = Event;
    type Error = Error;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        match &mut self.receive_state {
            ServerReceiveState::Command(state) => state.enqueue_input(bytes),
            ServerReceiveState::AuthenticateData(state) => state.enqueue_input(bytes),
            ServerReceiveState::IdleAccept(state) => state.enqueue_input(bytes),
            ServerReceiveState::IdleDone(state) => state.enqueue_input(bytes),
            ServerReceiveState::Dummy => unreachable!(),
        }
    }

    fn next(&mut self) -> Result<Self::Event, Interrupt<Self::Error>> {
        loop {
            if let Some(event) = self.progress_send()? {
                return Ok(event);
            }

            if let Some(event) = self.progress_receive()? {
                return Ok(event);
            }
        }
    }
}

/// Handle for enqueued [`Response`].
///
/// This handle can be used to track the sending progress. After a [`Response`] was enqueued via
/// [`Server::enqueue_data`] or [`Server::enqueue_status`] it is in the process of being
/// sent until [`Server::next`] returns a [`Event::ResponseSent`] with the
/// corresponding handle.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct ResponseHandle(RawHandle);

// Implement a short debug representation that hides the underlying raw handle
impl Debug for ResponseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ResponseHandle")
            .field(&self.0.generator_id())
            .field(&self.0.handle_id())
            .finish()
    }
}

impl Handle for ResponseHandle {
    fn from_raw(raw_handle: RawHandle) -> Self {
        Self(raw_handle)
    }
}

#[derive(Debug)]
pub enum Event {
    /// Initial [`Greeting] was sent successfully.
    GreetingSent {
        greeting: Greeting<'static>,
    },
    /// Enqueued [`Response`] was sent successfully.
    ResponseSent {
        /// Handle of the formerly enqueued [`Response`].
        handle: ResponseHandle,
        /// Formerly enqueued [`Response`] that was now sent.
        response: Response<'static>,
    },
    /// Command received.
    CommandReceived {
        command: Command<'static>,
    },
    /// Command AUTHENTICATE received.
    ///
    /// Note: The server MUST call [`Server::authenticate_continue`] (if it needs more data for
    /// authentication) or [`Server::authenticate_finish`] (if there already is enough data for
    /// authentication) next. "Enough data" is determined by the used SASL mechanism, if there was
    /// an initial response (SASL-IR), etc.
    CommandAuthenticateReceived {
        command_authenticate: CommandAuthenticate,
    },
    /// Continuation to AUTHENTICATE received.
    ///
    /// Note: The server MUST call [`Server::authenticate_continue`] (if it needs more data for
    /// authentication) or [`Server::authenticate_finish`] (if there already is enough data for
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
pub enum Error {
    #[error("Expected `\\r\\n`, got `\\n`")]
    ExpectedCrlfGotLf { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Received malformed message")]
    MalformedMessage { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Literal was rejected because it was too long")]
    LiteralTooLong { discarded_bytes: Secret<Box<[u8]>> },
    #[error("Command is too long")]
    CommandTooLong { discarded_bytes: Secret<Box<[u8]>> },
}
