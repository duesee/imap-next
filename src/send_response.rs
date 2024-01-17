use std::{collections::VecDeque, fmt::Debug};

use bytes::BytesMut;
use imap_codec::encode::{Encoder, Fragment};

use crate::stream::{AnyStream, StreamError};

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
    ) -> Result<Option<(K, C::Message<'static>)>, StreamError> {
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
                        // Note: The server doesn't need to wait before sending a literal.
                        //       Thus, non-sync literals doesn't make sense here.
                        //       This is currently an issue in imap-codec,
                        //       see https://github.com/duesee/imap-codec/issues/332
                        Fragment::Literal { data, .. } => data,
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
        stream.write_all(&mut self.write_buffer).await?;

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
