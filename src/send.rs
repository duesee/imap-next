use std::{collections::VecDeque, fmt::Debug};

use bytes::BytesMut;
use imap_codec::{
    encode::{Encoder, Fragment},
    imap_types::{
        command::{Command, CommandBody},
        core::LiteralMode,
    },
    CommandCodec,
};
use tokio::io::AsyncWriteExt;

use crate::stream::AnyStream;

#[derive(Debug)]
pub struct SendCommandState<K> {
    codec: CommandCodec,
    // The commands that should be send.
    send_queue: VecDeque<SendCommandQueueEntry<K>>,
    // State of the command that is currently being sent.
    send_progress: Option<SendCommandProgress<K>>,
    // Used for writing the current command to the stream.
    // Should be empty if `send_progress` is `None`.
    write_buffer: BytesMut,
}

impl<K> SendCommandState<K> {
    pub fn new(codec: CommandCodec, write_buffer: BytesMut) -> Self {
        Self {
            codec,
            send_queue: VecDeque::new(),
            send_progress: None,
            write_buffer,
        }
    }

    pub fn enqueue(&mut self, key: K, command: Command<'static>) {
        let mut fragments: VecDeque<_> = self.codec.encode(&command).collect();

        // C: A1 IDLE\r\nDONE\r\n
        //    ^^^^^^^^^^^
        //    |          ^^^^^^^^
        //    |          |
        //    Command    Command Continuation
        //
        // IDLE is a command with a command continuation similar to literals.
        // Thus, we can reuse our literal machinery.
        if matches!(&command.body, CommandBody::Idle) {
            fragments.push_back(Fragment::Literal {
                data: b"DONE\r\n".to_vec(),
                mode: LiteralMode::Sync,
            });
        }

        let entry = SendCommandQueueEntry {
            key,
            command,
            fragments,
        };
        self.send_queue.push_back(entry);
    }

    pub fn command_in_progress(&self) -> Option<&Command<'static>> {
        self.send_progress.as_ref().map(|x| &x.command)
    }

    pub fn is_literal_on_hold(&self) -> bool {
        if let Some(progress) = &self.send_progress {
            if let Some(literal) = &progress.next_literal {
                return literal.on_hold;
            }
        }

        false
    }

    pub fn release_last_literal(&mut self) {
        if let Some(progress) = &mut self.send_progress {
            if let Some(literal) = &mut progress.next_literal {
                literal.on_hold = false;
            }
        }
    }

    pub fn abort_command(&mut self) -> Option<(K, Command<'static>)> {
        self.write_buffer.clear();
        self.send_progress
            .take()
            .map(|progress| (progress.key, progress.command))
    }

    pub fn continue_command(&mut self) -> bool {
        let Some(write_progress) = self.send_progress.as_mut() else {
            return false;
        };
        let Some(literal_progress) = write_progress.next_literal.as_mut() else {
            return false;
        };
        if literal_progress.received_continue {
            return false;
        }

        literal_progress.received_continue = true;

        true
    }

    pub async fn progress(
        &mut self,
        stream: &mut AnyStream,
    ) -> Result<Option<(K, Command<'static>)>, tokio::io::Error> {
        let progress = match self.send_progress.take() {
            Some(progress) => {
                // We are currently sending a command to the server. This sending process was
                // previously aborted for one of two reasons: Either we needed to wait for a
                // `Continue` from the server or the `Future` was dropped while sending.
                progress
            }
            None => {
                let Some(entry) = self.send_queue.pop_front() else {
                    // There is currently no command that need to be sent
                    return Ok(None);
                };

                // Start sending the next command
                SendCommandProgress {
                    key: entry.key,
                    command: entry.command,
                    next_literal: None,
                    next_fragments: entry.fragments,
                }
            }
        };
        let progress = self.send_progress.insert(progress);

        // Handle the outstanding literal first if there is one
        if let Some(literal_progress) = progress.next_literal.take() {
            if literal_progress.received_continue && !literal_progress.on_hold {
                // We received a `Continue` from the server, and are not holding the literal back.
                // We can send the literal now.
                self.write_buffer.extend(literal_progress.data);
            } else {
                // Delay this literal because we still wait for the `Continue` from the server
                progress.next_literal = Some(literal_progress);

                // Make sure that the line before the literal is sent completely to the server
                stream.0.write_all_buf(&mut self.write_buffer).await?;

                return Ok(None);
            }
        }

        // Handle the outstanding lines or literals
        let need_continue = loop {
            if let Some(fragment) = progress.next_fragments.pop_front() {
                match fragment {
                    Fragment::Line { data } => {
                        self.write_buffer.extend(data);
                    }
                    Fragment::Literal { data, mode: _mode } => {
                        let on_hold = match &progress.command.body {
                            CommandBody::Idle => true,
                            _ => false,
                        };

                        // TODO: Handle `LITERAL{+,-}`.
                        // Delay this literal because we need to wait for a `Continue` from
                        // the server
                        progress.next_literal = Some(SendCommandLiteralProgress {
                            data,
                            received_continue: false,
                            on_hold,
                        });
                        break true;
                    }
                    Fragment::AuthData { .. } => {
                        unimplemented!()
                    }
                }
            } else {
                break false;
            }
        };

        // Send the bytes of the command to the server
        stream.0.write_all_buf(&mut self.write_buffer).await?;

        if need_continue {
            Ok(None)
        } else {
            // Command was sent completely
            Ok(self
                .send_progress
                .take()
                .map(|progress| (progress.key, progress.command)))
        }
    }
}

#[derive(Debug)]
struct SendCommandQueueEntry<K> {
    key: K,
    command: Command<'static>,
    fragments: VecDeque<Fragment>,
}

#[derive(Debug)]
struct SendCommandProgress<K> {
    key: K,
    command: Command<'static>,
    // If defined this literal need to be sent before `next_fragments`.
    next_literal: Option<SendCommandLiteralProgress>,
    // The fragments that need to be sent.
    next_fragments: VecDeque<Fragment>,
}

#[derive(Debug)]
struct SendCommandLiteralProgress {
    // The bytes of the literal.
    data: Vec<u8>,
    // Was the literal already acknowledged by a `Continue` from the server?
    received_continue: bool,
    // Was the literal already acknowledged by us?
    // Note: This artificial delay is useful to implement IDLE.
    on_hold: bool,
}

#[derive(Debug)]
pub struct SendResponseState<C: Encoder, K>
where
    C::Message<'static>: Debug,
{
    codec: C,
    // The responses that should be sent.
    send_queue: VecDeque<SendResponseQueueEntry<C, K>>,
    // State of the response that is currently being sent.
    send_progress: Option<SendResponseProgress<C, K>>,
    // Used for writing the current response to the stream.
    // Should be empty if `send_in_progress_key` is `None`.
    write_buffer: BytesMut,
}

impl<C: Encoder, K> SendResponseState<C, K>
where
    C::Message<'static>: Debug,
{
    pub fn new(codec: C, write_buffer: BytesMut) -> Self {
        Self {
            codec,
            send_queue: VecDeque::new(),
            send_progress: None,
            write_buffer,
        }
    }

    pub fn enqueue(&mut self, key: K, response: C::Message<'static>) {
        let fragments = self.codec.encode(&response).collect();
        let entry = SendResponseQueueEntry {
            key,
            response,
            fragments,
        };
        self.send_queue.push_back(entry);
    }

    pub fn finish(mut self) -> BytesMut {
        self.write_buffer.clear();
        self.write_buffer
    }

    pub async fn progress(
        &mut self,
        stream: &mut AnyStream,
    ) -> Result<Option<(K, C::Message<'static>)>, tokio::io::Error> {
        let progress = match self.send_progress.take() {
            Some(progress) => {
                // We are currently sending a response. This sending process was
                // previously aborted because the `Future` was dropped while sending.
                progress
            }
            None => {
                let Some(entry) = self.send_queue.pop_front() else {
                    // There is currently no response that need to be sent
                    return Ok(None);
                };

                // Push the response to the write buffer
                for fragment in entry.fragments {
                    let data = match fragment {
                        Fragment::Line { data } => data,
                        // TODO: Handle `LITERAL{+,-}`.
                        Fragment::Literal { data, mode: _mode } => data,
                        Fragment::AuthData { data } => data,
                    };
                    self.write_buffer.extend(data);
                }

                SendResponseProgress {
                    key: entry.key,
                    response: entry.response,
                }
            }
        };
        self.send_progress = Some(progress);

        // Send all bytes of current response
        stream.0.write_all_buf(&mut self.write_buffer).await?;

        // Response was sent completely
        Ok(self
            .send_progress
            .take()
            .map(|progress| (progress.key, progress.response)))
    }
}

#[derive(Debug)]
struct SendResponseQueueEntry<C: Encoder, K>
where
    C::Message<'static>: Debug,
{
    key: K,
    response: C::Message<'static>,
    fragments: Vec<Fragment>,
}

#[derive(Debug)]
struct SendResponseProgress<C: Encoder, K>
where
    C::Message<'static>: Debug,
{
    key: K,
    response: C::Message<'static>,
}
