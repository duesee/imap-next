use std::{collections::VecDeque, convert::Infallible};

use imap_codec::{
    encode::{Encoder, Fragment},
    AuthenticateDataCodec, CommandCodec, IdleDoneCodec,
};
use imap_types::{
    auth::AuthenticateData,
    command::{Command, CommandBody},
    core::{LiteralMode, Tag},
    extensions::idle::IdleDone,
    response::{Status, StatusBody, StatusKind, Tagged},
};
use tracing::warn;

use crate::{client::ClientFlowCommandHandle, types::CommandAuthenticate, FlowInterrupt, FlowIo};

pub struct ClientSendState {
    command_codec: CommandCodec,
    authenticate_data_codec: AuthenticateDataCodec,
    idle_done_codec: IdleDoneCodec,
    /// FIFO queue for messages that should be sent next.
    queued_messages: VecDeque<QueuedMessage>,
    /// Message that is currently being sent.
    current_message: Option<CurrentMessage>,
}

impl ClientSendState {
    pub fn new(
        command_codec: CommandCodec,
        authenticate_data_codec: AuthenticateDataCodec,
        idle_done_codec: IdleDoneCodec,
    ) -> Self {
        Self {
            command_codec,
            authenticate_data_codec,
            idle_done_codec,
            queued_messages: VecDeque::new(),
            current_message: None,
        }
    }

    pub fn enqueue_command(&mut self, handle: ClientFlowCommandHandle, command: Command<'static>) {
        self.queued_messages
            .push_back(QueuedMessage { handle, command });
    }

    /// Terminates the current message depending on the received status.
    pub fn maybe_terminate(&mut self, status: &Status) -> Option<ClientSendTermination> {
        // TODO: Do we want more checks on the state? Was idle already accepted? Does the command even has a literal? etc.
        // If we reach one of the return statements, the current message will be removed
        let current_message = self.current_message.take()?;
        self.current_message = Some(match current_message {
            CurrentMessage::Command(state) => {
                // Check if status matches the current command
                if let Status::Tagged(Tagged {
                    tag,
                    body: StatusBody { kind, .. },
                    ..
                }) = status
                {
                    if *kind == StatusKind::Bad && tag == &state.command.tag {
                        // Terminate command because literal was rejected
                        return Some(ClientSendTermination::LiteralRejected {
                            handle: state.handle,
                            command: state.command,
                        });
                    }
                }

                CurrentMessage::Command(state)
            }
            CurrentMessage::Authenticate(state) => {
                // Check if status matches the current authenticate command
                if let Status::Tagged(Tagged {
                    tag,
                    body: StatusBody { kind, .. },
                    ..
                }) = status
                {
                    if tag == &state.command_authenticate.tag {
                        match kind {
                            StatusKind::Ok => {
                                // Terminate authenticate command because it was accepted
                                return Some(ClientSendTermination::AuthenticateAccepted {
                                    handle: state.handle,
                                    command_authenticate: state.command_authenticate,
                                });
                            }
                            StatusKind::No | StatusKind::Bad => {
                                // Terminate authenticate command because it was rejected
                                return Some(ClientSendTermination::AuthenticateRejected {
                                    handle: state.handle,
                                    command_authenticate: state.command_authenticate,
                                });
                            }
                        };
                    }
                }

                CurrentMessage::Authenticate(state)
            }
            CurrentMessage::Idle(state) => {
                // Check if status matches the current idle command
                if let Status::Tagged(Tagged {
                    tag,
                    body: StatusBody { kind, .. },
                    ..
                }) = status
                {
                    if tag == &state.tag {
                        if matches!(kind, StatusKind::Ok | StatusKind::Bad) {
                            warn!(got=?status, "Expected command continuation request response or NO command completion result");
                            warn!("Interpreting as IDLE rejected");
                        }

                        // Terminate idle command because it was rejected
                        return Some(ClientSendTermination::IdleRejected {
                            handle: state.handle,
                        });
                    }
                }

                CurrentMessage::Idle(state)
            }
        });

        None
    }

    /// Handles the received continuation request for a literal.
    pub fn literal_continue(&mut self) -> bool {
        // Check whether in correct state
        let Some(current_message) = self.current_message.take() else {
            return false;
        };
        let CurrentMessage::Command(state) = current_message else {
            self.current_message = Some(current_message);
            return false;
        };
        let CommandActivity::WaitingForLiteralAccepted { limbo_literal } = state.activity else {
            self.current_message = Some(CurrentMessage::Command(state));
            return false;
        };

        // Change state
        self.current_message = Some(CurrentMessage::Command(CommandState {
            activity: CommandActivity::PushingFragments {
                accepted_literal: Some(limbo_literal),
            },
            ..state
        }));

        true
    }

    /// Handles the received continuation request for an authenticate data.
    pub fn authenticate_continue(&mut self) -> Option<ClientFlowCommandHandle> {
        // Check whether in correct state
        let current_message = self.current_message.take()?;
        let CurrentMessage::Authenticate(state) = current_message else {
            self.current_message = Some(current_message);
            return None;
        };
        let AuthenticateActivity::WaitingForAuthenticateResponse = state.activity else {
            self.current_message = Some(CurrentMessage::Authenticate(state));
            return None;
        };

        // Change state
        self.current_message = Some(CurrentMessage::Authenticate(AuthenticateState {
            activity: AuthenticateActivity::WaitingForAuthenticateDataSet,
            ..state
        }));

        Some(state.handle)
    }

    /// Takes the requested authenticate data and sends it to the server.
    pub fn set_authenticate_data(
        &mut self,
        authenticate_data: AuthenticateData<'static>,
    ) -> Result<ClientFlowCommandHandle, AuthenticateData<'static>> {
        // Check whether in correct state
        let Some(current_message) = self.current_message.take() else {
            return Err(authenticate_data);
        };
        let CurrentMessage::Authenticate(state) = current_message else {
            self.current_message = Some(current_message);
            return Err(authenticate_data);
        };
        let AuthenticateActivity::WaitingForAuthenticateDataSet = state.activity else {
            self.current_message = Some(CurrentMessage::Authenticate(state));
            return Err(authenticate_data);
        };

        // Encode authenticate data
        let mut fragments = self.authenticate_data_codec.encode(&authenticate_data);
        // Authenticate data is a single line by definition
        let Some(Fragment::Line {
            data: authenticate_data,
        }) = fragments.next()
        else {
            unreachable!()
        };
        assert!(fragments.next().is_none());

        // Change state
        self.current_message = Some(CurrentMessage::Authenticate(AuthenticateState {
            activity: AuthenticateActivity::PushingAuthenticateData { authenticate_data },
            ..state
        }));

        Ok(state.handle)
    }

    /// Handles the received continuation request for the idle done.
    pub fn idle_continue(&mut self) -> Option<ClientFlowCommandHandle> {
        // Check whether in correct state
        let current_message = self.current_message.take()?;
        let CurrentMessage::Idle(state) = current_message else {
            self.current_message = Some(current_message);
            return None;
        };
        let IdleActivity::WaitingForIdleResponse = state.activity else {
            self.current_message = Some(CurrentMessage::Idle(state));
            return None;
        };

        // Change state
        self.current_message = Some(CurrentMessage::Idle(IdleState {
            activity: IdleActivity::WaitingForIdleDoneSet,
            ..state
        }));

        Some(state.handle)
    }

    /// Sends the requested idle done to the server.
    pub fn set_idle_done(&mut self) -> Option<ClientFlowCommandHandle> {
        // Check whether in correct state
        let current_message = self.current_message.take()?;
        let CurrentMessage::Idle(state) = current_message else {
            self.current_message = Some(current_message);
            return None;
        };
        let IdleActivity::WaitingForIdleDoneSet = state.activity else {
            self.current_message = Some(CurrentMessage::Idle(state));
            return None;
        };

        // Encode idle done
        let mut fragments = self.idle_done_codec.encode(&IdleDone);
        // Idle done is a single line by defintion
        let Some(Fragment::Line {
            data: idle_done, ..
        }) = fragments.next()
        else {
            unreachable!()
        };
        assert!(fragments.next().is_none());

        // Change state
        let handle = state.handle;
        self.current_message = Some(CurrentMessage::Idle(IdleState {
            activity: IdleActivity::PushingIdleDone { idle_done },
            ..state
        }));

        Some(handle)
    }

    pub fn progress(&mut self) -> Result<Option<ClientSendEvent>, FlowInterrupt<Infallible>> {
        let current_message = match self.current_message.take() {
            Some(current_message) => {
                // We are currently sending a message but the sending process was aborted for one
                // of these reasons:
                // - The flow was interrupted
                // - The server must send a continuation request or a status
                // - The client flow user must provide more data
                // Continue the sending process.
                current_message
            }
            None => {
                let Some(queued_message) = self.queued_messages.pop_front() else {
                    // There is currently no message that needs to be sent
                    return Ok(None);
                };

                queued_message.start(&self.command_codec)
            }
        };

        // Creates a buffer for writing the current message
        let mut write_buffer = Vec::new();

        // Push as many bytes of the message as possible to the buffer
        let current_message = current_message.push_to_buffer(&mut write_buffer);

        if write_buffer.is_empty() {
            // Inform the state of the current message that all bytes are sent
            match current_message.finish_sending() {
                FinishSendingResult::Uncompleted {
                    state: current_message,
                    event,
                } => {
                    // Message is not finished yet
                    self.current_message = Some(current_message);
                    Ok(event)
                }
                FinishSendingResult::Completed { event } => {
                    // Message was sent completely
                    Ok(Some(event))
                }
            }
        } else {
            // Store the current message, we'll continue later
            self.current_message = Some(current_message);

            // Interrupt the flow for sending all bytes of current message
            Err(FlowInterrupt::Io(FlowIo::Output(write_buffer)))
        }
    }
}

/// Queued (and not sent yet) message.
struct QueuedMessage {
    handle: ClientFlowCommandHandle,
    command: Command<'static>,
}

impl QueuedMessage {
    /// Start the sending process for this message.
    fn start(self, codec: &CommandCodec) -> CurrentMessage {
        let handle = self.handle;
        let command = self.command;
        let mut fragments = codec.encode(&command);
        let tag = command.tag;

        match command.body {
            CommandBody::Authenticate {
                mechanism,
                initial_response,
            } => {
                // The authenticate command is a single line by definition
                let Some(Fragment::Line { data: authenticate }) = fragments.next() else {
                    unreachable!()
                };
                assert!(fragments.next().is_none());

                CurrentMessage::Authenticate(AuthenticateState {
                    handle,
                    command_authenticate: CommandAuthenticate {
                        tag,
                        mechanism,
                        initial_response,
                    },
                    activity: AuthenticateActivity::PushingAuthenticate { authenticate },
                })
            }
            CommandBody::Idle => {
                // The idle command is a single line by definition
                let Some(Fragment::Line { data: idle }) = fragments.next() else {
                    unreachable!()
                };
                assert!(fragments.next().is_none());

                CurrentMessage::Idle(IdleState {
                    handle,
                    tag,
                    activity: IdleActivity::PushingIdle { idle },
                })
            }
            body => CurrentMessage::Command(CommandState {
                handle,
                command: Command { tag, body },
                fragments: fragments.collect(),
                activity: CommandActivity::PushingFragments {
                    accepted_literal: None,
                },
            }),
        }
    }
}

/// Currently being sent message.
enum CurrentMessage {
    /// Sending state of regular command.
    Command(CommandState),
    /// Sending state of authenticate command.
    Authenticate(AuthenticateState),
    /// Sending state of idle command.
    Idle(IdleState),
}

impl CurrentMessage {
    /// Pushes as many bytes as possible from the message to the buffer.
    fn push_to_buffer(self, write_buffer: &mut Vec<u8>) -> Self {
        match self {
            Self::Command(state) => Self::Command(state.push_to_buffer(write_buffer)),
            Self::Authenticate(state) => Self::Authenticate(state.push_to_buffer(write_buffer)),
            Self::Idle(state) => Self::Idle(state.push_to_buffer(write_buffer)),
        }
    }

    /// Updates the state after all bytes were sent.
    fn finish_sending(self) -> FinishSendingResult<Self> {
        match self {
            Self::Command(state) => state.finish_sending().map_state(Self::Command),
            Self::Authenticate(state) => state.finish_sending().map_state(Self::Authenticate),
            Self::Idle(state) => state.finish_sending().map_state(Self::Idle),
        }
    }
}

/// Updated message state after sending all bytes, see `finish_sending`.
enum FinishSendingResult<S> {
    /// Message not finished yet.
    Uncompleted {
        /// Updated message state.
        state: S,
        /// Event that needs to be returned by `progress`.
        event: Option<ClientSendEvent>,
    },
    /// Message sent completely.
    Completed {
        /// Event that needs to be returned by `progress`.
        event: ClientSendEvent,
    },
}

impl<S> FinishSendingResult<S> {
    fn map_state<T>(self, f: impl Fn(S) -> T) -> FinishSendingResult<T> {
        match self {
            FinishSendingResult::Uncompleted { state, event } => FinishSendingResult::Uncompleted {
                state: f(state),
                event,
            },
            FinishSendingResult::Completed { event } => FinishSendingResult::Completed { event },
        }
    }
}

struct CommandState {
    handle: ClientFlowCommandHandle,
    command: Command<'static>,
    /// Outstanding command fragments that needs to be sent.
    fragments: VecDeque<Fragment>,
    activity: CommandActivity,
}

impl CommandState {
    fn push_to_buffer(self, write_buffer: &mut Vec<u8>) -> Self {
        let mut fragments = self.fragments;
        let activity = match self.activity {
            CommandActivity::PushingFragments { accepted_literal } => {
                // First push the accepted literal if available
                if let Some(data) = accepted_literal {
                    write_buffer.extend(data);
                }

                // Push as many fragments as possible
                let limbo_literal = loop {
                    match fragments.pop_front() {
                        Some(
                            Fragment::Line { data }
                            | Fragment::Literal {
                                data,
                                mode: LiteralMode::NonSync,
                            },
                        ) => {
                            write_buffer.extend(data);
                        }
                        Some(Fragment::Literal {
                            data,
                            mode: LiteralMode::Sync,
                        }) => {
                            // Stop pushing fragments because a literal needs to be accepted
                            // by the server
                            break Some(data);
                        }
                        None => break None,
                    };
                };

                // Done with pushing
                CommandActivity::WaitingForFragmentsSent { limbo_literal }
            }
            activity => activity,
        };

        Self {
            fragments,
            activity,
            ..self
        }
    }

    fn finish_sending(self) -> FinishSendingResult<Self> {
        match self.activity {
            CommandActivity::WaitingForFragmentsSent { limbo_literal } => match limbo_literal {
                Some(limbo_literal) => FinishSendingResult::Uncompleted {
                    state: Self {
                        activity: CommandActivity::WaitingForLiteralAccepted { limbo_literal },
                        ..self
                    },
                    event: None,
                },
                None => FinishSendingResult::Completed {
                    event: ClientSendEvent::Command {
                        handle: self.handle,
                        command: self.command,
                    },
                },
            },
            activity => FinishSendingResult::Uncompleted {
                state: Self { activity, ..self },
                event: None,
            },
        }
    }
}

enum CommandActivity {
    /// Pushing fragments to the write buffer.
    PushingFragments {
        /// Literal that was accepted by the server and needs to be sent before the fragments.
        accepted_literal: Option<Vec<u8>>,
    },
    /// Waiting until the pushed fragments are sent.
    WaitingForFragmentsSent {
        /// Literal that needs to be accepted by the server after the pushed fragments are sent.
        limbo_literal: Option<Vec<u8>>,
    },
    /// Waiting until the server accepts the literal via continuation request or rejects it
    /// via status.
    WaitingForLiteralAccepted {
        /// Literal that needs to be accepted by the server.
        limbo_literal: Vec<u8>,
    },
}

struct AuthenticateState {
    handle: ClientFlowCommandHandle,
    command_authenticate: CommandAuthenticate,
    activity: AuthenticateActivity,
}

impl AuthenticateState {
    fn push_to_buffer(self, write_buffer: &mut Vec<u8>) -> Self {
        let activity = match self.activity {
            AuthenticateActivity::PushingAuthenticate { authenticate } => {
                write_buffer.extend(authenticate);
                AuthenticateActivity::WaitingForAuthenticateSent
            }
            AuthenticateActivity::PushingAuthenticateData { authenticate_data } => {
                write_buffer.extend(authenticate_data);
                AuthenticateActivity::WaitingForAuthenticateDataSent
            }
            activity => activity,
        };

        Self { activity, ..self }
    }

    fn finish_sending(self) -> FinishSendingResult<Self> {
        match self.activity {
            AuthenticateActivity::WaitingForAuthenticateSent => FinishSendingResult::Uncompleted {
                state: Self {
                    activity: AuthenticateActivity::WaitingForAuthenticateResponse,
                    ..self
                },
                event: Some(ClientSendEvent::Authenticate {
                    handle: self.handle,
                }),
            },
            AuthenticateActivity::WaitingForAuthenticateDataSent => {
                FinishSendingResult::Uncompleted {
                    state: Self {
                        activity: AuthenticateActivity::WaitingForAuthenticateResponse,
                        ..self
                    },
                    event: None,
                }
            }
            activity => FinishSendingResult::Uncompleted {
                state: Self { activity, ..self },
                event: None,
            },
        }
    }
}

enum AuthenticateActivity {
    /// Pushing the authenticate command to the write buffer.
    PushingAuthenticate { authenticate: Vec<u8> },
    /// Waiting until the pushed authenticate command is sent.
    WaitingForAuthenticateSent,
    /// Waiting until the server requests more authenticate data via continuation request or
    /// accepts/rejects the authenticate command via status.
    WaitingForAuthenticateResponse,
    /// Waiting until the client flow user provides the authenticate data.
    ///
    /// Specifically, [`ClientFlow::set_authenticate_data`](crate::client::ClientFlow::set_authenticate_data).
    WaitingForAuthenticateDataSet,
    /// Pushing the authenticate data to the write buffer.
    PushingAuthenticateData { authenticate_data: Vec<u8> },
    /// Waiting until the pushed authenticate data is sent.
    WaitingForAuthenticateDataSent,
}

struct IdleState {
    handle: ClientFlowCommandHandle,
    tag: Tag<'static>,
    activity: IdleActivity,
}

impl IdleState {
    fn push_to_buffer(self, write_buffer: &mut Vec<u8>) -> Self {
        let activity = match self.activity {
            IdleActivity::PushingIdle { idle } => {
                write_buffer.extend(idle);
                IdleActivity::WaitingForIdleSent
            }
            IdleActivity::PushingIdleDone { idle_done } => {
                write_buffer.extend(idle_done);
                IdleActivity::WaitingForIdleDoneSent
            }
            activity => activity,
        };

        Self { activity, ..self }
    }

    fn finish_sending(self) -> FinishSendingResult<Self> {
        match self.activity {
            IdleActivity::WaitingForIdleSent => FinishSendingResult::Uncompleted {
                state: Self {
                    activity: IdleActivity::WaitingForIdleResponse,
                    ..self
                },
                event: Some(ClientSendEvent::Idle {
                    handle: self.handle,
                }),
            },
            IdleActivity::WaitingForIdleDoneSent => FinishSendingResult::Completed {
                event: ClientSendEvent::IdleDone {
                    handle: self.handle,
                },
            },
            activity => FinishSendingResult::Uncompleted {
                state: Self { activity, ..self },
                event: None,
            },
        }
    }
}

enum IdleActivity {
    /// Pushing the idle command to the write buffer.
    PushingIdle { idle: Vec<u8> },
    /// Waiting until the pushed idle command is sent.
    WaitingForIdleSent,
    /// Waiting until the server accepts the idle command via continuation request or rejects it
    /// via status.
    WaitingForIdleResponse,
    /// Waiting until the client flow user triggers idle done.
    ///
    /// Specifically, [`ClientFlow::set_idle_done`](crate::client::ClientFlow::set_idle_done).
    WaitingForIdleDoneSet,
    /// Pushing the idle done to the write buffer.
    PushingIdleDone { idle_done: Vec<u8> },
    /// Waiting until the pushed idle done is sent.
    WaitingForIdleDoneSent,
}

/// Message sent.
pub enum ClientSendEvent {
    Command {
        handle: ClientFlowCommandHandle,
        command: Command<'static>,
    },
    Authenticate {
        handle: ClientFlowCommandHandle,
    },
    Idle {
        handle: ClientFlowCommandHandle,
    },
    IdleDone {
        handle: ClientFlowCommandHandle,
    },
}

/// Message was terminated via [`ClientSendState::maybe_terminate`].
pub enum ClientSendTermination {
    /// Command was terminated because its literal was rejected by the server.
    LiteralRejected {
        handle: ClientFlowCommandHandle,
        command: Command<'static>,
    },
    /// Authenticate command was accepted.
    AuthenticateAccepted {
        handle: ClientFlowCommandHandle,
        command_authenticate: CommandAuthenticate,
    },
    /// Authenticate command was rejected.
    AuthenticateRejected {
        handle: ClientFlowCommandHandle,
        command_authenticate: CommandAuthenticate,
    },
    /// Idle command was rejected.
    IdleRejected { handle: ClientFlowCommandHandle },
}
