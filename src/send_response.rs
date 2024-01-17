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
    // FIFO queue for responses that should be sent next.
    queued_responses: VecDeque<QueuedResponse<C, K>>,
    // The response that is currently being sent.
    current_response: Option<CurrentResponse<C, K>>,
    // Used for writing the current response to the stream.
    // Should be empty if `current_response` is `None`.
    write_buffer: BytesMut,
}

impl<C: Encoder, K> SendResponseState<C, K>
where
    C::Message<'static>: Debug,
{
    pub fn new(codec: C, write_buffer: BytesMut) -> Self {
        Self {
            codec,
            queued_responses: VecDeque::new(),
            current_response: None,
            write_buffer,
        }
    }

    pub fn enqueue(&mut self, key: K, response: C::Message<'static>) {
        let fragments = self.codec.encode(&response).collect();
        let entry = QueuedResponse {
            key,
            response,
            fragments,
        };
        self.queued_responses.push_back(entry);
    }

    pub fn finish(mut self) -> BytesMut {
        self.write_buffer.clear();
        self.write_buffer
    }

    pub async fn progress(
        &mut self,
        stream: &mut AnyStream,
    ) -> Result<Option<SendResponseEvent<C, K>>, StreamError> {
        let current_response = match self.current_response.take() {
            Some(current_response) => {
                // We are currently sending a response but the sending process was cancelled.
                // Continue the sending process.
                current_response
            }
            None => {
                assert!(self.write_buffer.is_empty());

                let Some(queued_response) = self.queued_responses.pop_front() else {
                    // There is currently no response that need to be sent
                    return Ok(None);
                };

                queued_response.push_to_buffer(&mut self.write_buffer)
            }
        };

        // Store the current response to ensure cancellation safety
        self.current_response = Some(current_response);

        // Send all bytes of current response
        stream.write_all(&mut self.write_buffer).await?;

        // Restore the current response, can't fail because we set it to `Some` above
        let current_response = self.current_response.take().unwrap();

        // We finished sending a response completely
        Ok(Some(SendResponseEvent {
            key: current_response.key,
            response: current_response.response,
        }))
    }
}

// A response that is queued but not sent yet.
#[derive(Debug)]
struct QueuedResponse<C: Encoder, K>
where
    C::Message<'static>: Debug,
{
    key: K,
    response: C::Message<'static>,
    fragments: Vec<Fragment>,
}

impl<C: Encoder, K> QueuedResponse<C, K>
where
    C::Message<'static>: Debug,
{
    fn push_to_buffer(self, write_buffer: &mut BytesMut) -> CurrentResponse<C, K> {
        for fragment in self.fragments {
            let data = match fragment {
                Fragment::Line { data } => data,
                // Note: The server doesn't need to wait before sending a literal.
                //       Thus, non-sync literals doesn't make sense here.
                //       This is currently an issue in imap-codec,
                //       see https://github.com/duesee/imap-codec/issues/332
                Fragment::Literal { data, .. } => data,
            };
            write_buffer.extend(data);
        }

        CurrentResponse {
            key: self.key,
            response: self.response,
        }
    }
}

// A response that is currently being sent.
#[derive(Debug)]
struct CurrentResponse<C: Encoder, K>
where
    C::Message<'static>: Debug,
{
    key: K,
    response: C::Message<'static>,
}

// A response was sent.
pub struct SendResponseEvent<C: Encoder, K> {
    pub key: K,
    pub response: C::Message<'static>,
}
