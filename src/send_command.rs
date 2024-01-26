use std::{collections::VecDeque, fmt::Debug};

use bytes::BytesMut;
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

use crate::{
    client::ClientFlow,
    stream::{AnyStream, StreamError},
    types::CommandAuthenticate,
};

#[derive(Debug)]
pub struct SendCommandState<K: Copy> {
    command_codec: CommandCodec,
    authenticate_data_codec: AuthenticateDataCodec,
    idle_done_codec: IdleDoneCodec,
    /// FIFO queue for commands that should be sent next.
    queued_commands: VecDeque<QueuedCommand<K>>,
    /// The command that is currently being sent.
    current_command: Option<CurrentCommand<K>>,
    /// Used for writing the current command to the stream.
    /// Note that this buffer can be non-empty even if `current_command` is `None`
    /// because commands can be aborted (see `maybe_terminate`) but partially sent
    /// fragment must never be aborted.
    write_buffer: BytesMut,
}

impl<K: Copy> SendCommandState<K> {
    pub fn new(
        command_codec: CommandCodec,
        authenticate_data_codec: AuthenticateDataCodec,
        idle_done_codec: IdleDoneCodec,
        write_buffer: BytesMut,
    ) -> Self {
        Self {
            command_codec,
            authenticate_data_codec,
            idle_done_codec,
            queued_commands: VecDeque::new(),
            current_command: None,
            write_buffer,
        }
    }

    pub fn enqueue(&mut self, key: K, command: Command<'static>) {
        self.queued_commands
            .push_back(QueuedCommand { key, command });
    }

    /// Terminates the current command depending on the received status.
    pub fn maybe_remove(&mut self, status: &Status) -> Option<SendCommandTermination<K>> {
        // TODO: Do we want more checks on the state? Was idle already accepted? Does the command even has a literal? etc.
        // If we reach one of the return statements, the current command will be removed
        let current_command = self.current_command.take()?;
        self.current_command = Some(match current_command {
            CurrentCommand::Command(state) => {
                // Check if status matches the current command
                if let Status::Tagged(Tagged {
                    tag,
                    body: StatusBody { kind, .. },
                    ..
                }) = status
                {
                    if *kind == StatusKind::Bad && tag == &state.command.tag {
                        // Terminate command because literal was rejected
                        return Some(SendCommandTermination::LiteralRejected {
                            key: state.key,
                            command: state.command,
                        });
                    }
                }

                CurrentCommand::Command(state)
            }
            CurrentCommand::Authenticate(state) => {
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
                                return Some(SendCommandTermination::AuthenticateAccepted {
                                    key: state.key,
                                    command_authenticate: state.command_authenticate,
                                });
                            }
                            StatusKind::No | StatusKind::Bad => {
                                // Terminate authenticate command because it was rejected
                                return Some(SendCommandTermination::AuthenticateRejected {
                                    key: state.key,
                                    command_authenticate: state.command_authenticate,
                                });
                            }
                        };
                    }
                }

                CurrentCommand::Authenticate(state)
            }
            CurrentCommand::Idle(state) => {
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
                        return Some(SendCommandTermination::IdleRejected { key: state.key });
                    }
                }

                CurrentCommand::Idle(state)
            }
        });

        None
    }

    /// Handles the received continuation request for a literal.
    pub fn literal_continue(&mut self) -> bool {
        // Check whether in correct state
        let Some(current_command) = self.current_command.take() else {
            return false;
        };
        let CurrentCommand::Command(state) = current_command else {
            self.current_command = Some(current_command);
            return false;
        };
        let CommandActivity::WaitingForLiteralAccepted { limbo_literal } = state.activity else {
            self.current_command = Some(CurrentCommand::Command(state));
            return false;
        };

        // Change state
        self.current_command = Some(CurrentCommand::Command(CommandState {
            activity: CommandActivity::PushingFragments {
                accepted_literal: Some(limbo_literal),
            },
            ..state
        }));

        true
    }

    /// Handles the received continuation request for an authenticate data.
    pub fn authenticate_continue(&mut self) -> Option<K> {
        // Check whether in correct state
        let Some(current_command) = self.current_command.take() else {
            return None;
        };
        let CurrentCommand::Authenticate(state) = current_command else {
            self.current_command = Some(current_command);
            return None;
        };
        let AuthenticateActivity::WaitingForAuthenticateResponse = state.activity else {
            self.current_command = Some(CurrentCommand::Authenticate(state));
            return None;
        };

        // Change state
        self.current_command = Some(CurrentCommand::Authenticate(AuthenticateState {
            activity: AuthenticateActivity::WaitingForAuthenticateDataSet,
            ..state
        }));

        Some(state.key)
    }

    /// Takes the requested authenticate data and sends it to the server.
    pub fn set_authenticate_data(
        &mut self,
        authenticate_data: AuthenticateData,
    ) -> Result<K, AuthenticateData> {
        // Check whether in correct state
        let Some(current_command) = self.current_command.take() else {
            return Err(authenticate_data);
        };
        let CurrentCommand::Authenticate(state) = current_command else {
            self.current_command = Some(current_command);
            return Err(authenticate_data);
        };
        let AuthenticateActivity::WaitingForAuthenticateDataSet = state.activity else {
            self.current_command = Some(CurrentCommand::Authenticate(state));
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
        self.current_command = Some(CurrentCommand::Authenticate(AuthenticateState {
            activity: AuthenticateActivity::PushingAuthenticateData { authenticate_data },
            ..state
        }));

        Ok(state.key)
    }

    /// Handles the received continuation request for the idle done.
    pub fn idle_continue(&mut self) -> Option<K> {
        // Check whether in correct state
        let Some(current_command) = self.current_command.take() else {
            return None;
        };
        let CurrentCommand::Idle(state) = current_command else {
            self.current_command = Some(current_command);
            return None;
        };
        let IdleActivity::WaitingForIdleResponse = state.activity else {
            self.current_command = Some(CurrentCommand::Idle(state));
            return None;
        };

        // Change state
        self.current_command = Some(CurrentCommand::Idle(IdleState {
            activity: IdleActivity::WaitingForIdleDoneSet,
            ..state
        }));

        Some(state.key)
    }

    /// Sends the requested idle done to the server.
    pub fn set_idle_done(&mut self) -> Option<K> {
        // Check whether in correct state
        let Some(current_command) = self.current_command.take() else {
            return None;
        };
        let CurrentCommand::Idle(state) = current_command else {
            self.current_command = Some(current_command);
            return None;
        };
        let IdleActivity::WaitingForIdleDoneSet = state.activity else {
            self.current_command = Some(CurrentCommand::Idle(state));
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
        let key = state.key;
        self.current_command = Some(CurrentCommand::Idle(IdleState {
            activity: IdleActivity::PushingIdleDone { idle_done },
            ..state
        }));

        Some(key)
    }

    pub async fn progress(
        &mut self,
        stream: &mut AnyStream,
    ) -> Result<Option<SendCommandEvent<K>>, StreamError> {
        let current_command = match self.current_command.take() {
            Some(current_command) => {
                // We are currently sending a command but the sending process was aborted for one
                // of these reasons:
                // - The future was cancelled
                // - The server must send a continuation request or a status
                // - The client flow user must provide more data
                // Continue the sending process.
                current_command
            }
            None => {
                let Some(queued_command) = self.queued_commands.pop_front() else {
                    // There is currently no command that needs to be sent
                    return Ok(None);
                };

                queued_command.start(&self.command_codec)
            }
        };

        // Push as many bytes of the command as possible to the buffer
        let current_command = current_command.push_to_buffer(&mut self.write_buffer);

        // Store the current command to ensure cancellation safety
        self.current_command = Some(current_command);

        // Send all bytes of current command
        stream.write_all(&mut self.write_buffer).await?;

        // Restore the current command, can't fail because we set it to `Some` above
        let current_command = self.current_command.take().unwrap();

        // Inform the state of the current command that all bytes were sent
        match current_command.finish_sending() {
            FinishSendingResult::Uncompleted {
                state: current_command,
                event,
            } => {
                // Command is not finshed yet
                self.current_command = Some(current_command);
                Ok(event)
            }
            FinishSendingResult::Completed { event } => {
                // Command was sent completely
                Ok(Some(event))
            }
        }
    }
}

/// A command that is queued but not sent yet.
#[derive(Debug)]
struct QueuedCommand<K: Copy> {
    key: K,
    command: Command<'static>,
}

impl<K: Copy> QueuedCommand<K> {
    /// Start the sending process for this command.
    fn start(self, codec: &CommandCodec) -> CurrentCommand<K> {
        let key = self.key;
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

                CurrentCommand::Authenticate(AuthenticateState {
                    key,
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

                CurrentCommand::Idle(IdleState {
                    key,
                    tag,
                    activity: IdleActivity::PushingIdle { idle },
                })
            }
            body => CurrentCommand::Command(CommandState {
                key,
                command: Command { tag, body },
                fragments: fragments.collect(),
                activity: CommandActivity::PushingFragments {
                    accepted_literal: None,
                },
            }),
        }
    }
}

/// A command that is currently being sent.
#[derive(Debug)]
enum CurrentCommand<K: Copy> {
    /// The sending state of a regular command.
    Command(CommandState<K>),
    /// The sending state of a authenticate command.
    Authenticate(AuthenticateState<K>),
    /// The sending state of a idle command.
    Idle(IdleState<K>),
}

impl<K: Copy> CurrentCommand<K> {
    /// Pushes as many bytes as possible from the command to the buffer.
    fn push_to_buffer(self, write_buffer: &mut BytesMut) -> Self {
        match self {
            Self::Command(state) => Self::Command(state.push_to_buffer(write_buffer)),
            Self::Authenticate(state) => Self::Authenticate(state.push_to_buffer(write_buffer)),
            Self::Idle(state) => Self::Idle(state.push_to_buffer(write_buffer)),
        }
    }

    /// Updates the state after all bytes were sent.
    fn finish_sending(self) -> FinishSendingResult<K, Self> {
        match self {
            Self::Command(state) => state.finish_sending().map_state(Self::Command),
            Self::Authenticate(state) => state.finish_sending().map_state(Self::Authenticate),
            Self::Idle(state) => state.finish_sending().map_state(Self::Idle),
        }
    }
}

/// The updated command state after sending all bytes, see `finish_sending`.
enum FinishSendingResult<K, S> {
    // The command is not finished yet.
    Uncompleted {
        // The updated command state.
        state: S,
        // An event that needs to be returned by `progress`.
        event: Option<SendCommandEvent<K>>,
    },
    // The command was sent completely.
    Completed {
        // An event that needs to be returned by `progress`.
        event: SendCommandEvent<K>,
    },
}

impl<K, S> FinishSendingResult<K, S> {
    fn map_state<T>(self, f: impl Fn(S) -> T) -> FinishSendingResult<K, T> {
        match self {
            FinishSendingResult::Uncompleted { state, event } => FinishSendingResult::Uncompleted {
                state: f(state),
                event,
            },
            FinishSendingResult::Completed { event } => FinishSendingResult::Completed { event },
        }
    }
}

#[derive(Debug)]
struct CommandState<K> {
    key: K,
    command: Command<'static>,
    /// The outstanding command fragments that needs to be sent.
    fragments: VecDeque<Fragment>,
    activity: CommandActivity,
}

impl<K> CommandState<K> {
    fn push_to_buffer(self, write_buffer: &mut BytesMut) -> Self {
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

    fn finish_sending(self) -> FinishSendingResult<K, Self> {
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
                    event: SendCommandEvent::Command {
                        key: self.key,
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

#[derive(Debug)]
enum CommandActivity {
    /// Pushing fragments to the write buffer.
    PushingFragments {
        /// A literal that was accepted by the server and needs to be sent before the fragments.
        accepted_literal: Option<Vec<u8>>,
    },
    /// Waiting until the pushed fragments are sent.
    WaitingForFragmentsSent {
        /// A literal that needs to be accepted by the server after the pushed fragments are sent.
        limbo_literal: Option<Vec<u8>>,
    },
    /// Waiting until the server accepts the literal via continuation request or rejects it
    /// via status.
    WaitingForLiteralAccepted {
        /// The literal that needs to be accepted by the server.
        limbo_literal: Vec<u8>,
    },
}

#[derive(Debug)]
struct AuthenticateState<K: Copy> {
    key: K,
    command_authenticate: CommandAuthenticate,
    activity: AuthenticateActivity,
}

impl<K: Copy> AuthenticateState<K> {
    fn push_to_buffer(self, write_buffer: &mut BytesMut) -> Self {
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

    fn finish_sending(self) -> FinishSendingResult<K, Self> {
        match self.activity {
            AuthenticateActivity::WaitingForAuthenticateSent => FinishSendingResult::Uncompleted {
                state: Self {
                    activity: AuthenticateActivity::WaitingForAuthenticateResponse,
                    ..self
                },
                event: Some(SendCommandEvent::CommandAuthenticate { key: self.key }),
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

#[derive(Debug)]
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
    /// Specifically, [`ClientFlow::set_authenticate_data`].
    WaitingForAuthenticateDataSet,
    /// Pushing the authenticate data to the write buffer.
    PushingAuthenticateData { authenticate_data: Vec<u8> },
    /// Waiting until the pushed authenticate data is sent.
    WaitingForAuthenticateDataSent,
}

#[derive(Debug)]
struct IdleState<K: Copy> {
    key: K,
    tag: Tag<'static>,
    activity: IdleActivity,
}

impl<K: Copy> IdleState<K> {
    fn push_to_buffer(self, write_buffer: &mut BytesMut) -> Self {
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

    fn finish_sending(self) -> FinishSendingResult<K, Self> {
        match self.activity {
            IdleActivity::WaitingForIdleSent => FinishSendingResult::Uncompleted {
                state: Self {
                    activity: IdleActivity::WaitingForIdleResponse,
                    ..self
                },
                event: Some(SendCommandEvent::CommandIdle { key: self.key }),
            },
            IdleActivity::WaitingForIdleDoneSent => FinishSendingResult::Completed {
                event: SendCommandEvent::IdleDone { key: self.key },
            },
            activity => FinishSendingResult::Uncompleted {
                state: Self { activity, ..self },
                event: None,
            },
        }
    }
}

#[derive(Debug)]
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
    /// Specifically, [`ClientFlow::set_idle_done`].
    WaitingForIdleDoneSet,
    /// Pushing the idle done to the write buffer.
    PushingIdleDone { idle_done: Vec<u8> },
    /// Waiting until the pushed idle done is sent.
    WaitingForIdleDoneSent,
}

// A command was sent.
#[derive(Debug)]
pub enum SendCommandEvent<K> {
    Command { key: K, command: Command<'static> },
    CommandAuthenticate { key: K },
    CommandIdle { key: K },
    IdleDone { key: K },
}

// A command was terminated via `maybe_terminate`.
pub enum SendCommandTermination<K> {
    // A command was terminated because its literal was rejected by the server.
    LiteralRejected {
        key: K,
        command: Command<'static>,
    },
    // An authenticate command was terminated because the server accepted it.
    AuthenticateAccepted {
        key: K,
        command_authenticate: CommandAuthenticate,
    },
    // An authenticate command was terminated because the server rejected it.
    AuthenticateRejected {
        key: K,
        command_authenticate: CommandAuthenticate,
    },
    // Ad idle command was terminated because the server rejected it.
    IdleRejected {
        key: K,
    },
}
