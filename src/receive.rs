use bounded_static::IntoBoundedStatic;
use bytes::{Buf, BytesMut};
use imap_codec::decode::Decoder;

use crate::{FlowInterrupt, FlowIo};

pub struct ReceiveState<C> {
    codec: C,
    crlf_relaxed: bool,
    max_message_size: Option<u32>,
    next_fragment: NextFragment,
    /// How many bytes in the parse buffer do we already have checked?
    /// This is important if we need multiple attempts to read from the underlying
    /// stream before the message is completely received.
    seen_bytes: usize,
    /// Used for reading the current message from the stream.
    /// Its length should always be equal to or greater than `seen_bytes`.
    read_buffer: BytesMut,
}

impl<C> ReceiveState<C> {
    pub fn new(codec: C, crlf_relaxed: bool, max_message_size: Option<u32>) -> Self {
        Self::with_read_buffer(codec, crlf_relaxed, max_message_size, BytesMut::default())
    }

    fn with_read_buffer(
        codec: C,
        crlf_relaxed: bool,
        max_message_size: Option<u32>,
        read_buffer: BytesMut,
    ) -> Self {
        Self {
            codec,
            crlf_relaxed,
            max_message_size,
            next_fragment: NextFragment::start_new_line(),
            seen_bytes: 0,
            read_buffer,
        }
    }

    pub fn enqueue_input(&mut self, bytes: &[u8]) {
        self.read_buffer.extend(bytes);
    }

    pub fn start_literal(&mut self, length: u32) {
        self.next_fragment = NextFragment::Literal { length };
        self.read_buffer.reserve(length as usize);
    }

    pub fn finish_message(&mut self) {
        self.read_buffer.advance(self.seen_bytes);
        self.seen_bytes = 0;
        self.next_fragment = NextFragment::start_new_line();
    }

    pub fn discard_message(&mut self) -> Box<[u8]> {
        let discarded_bytes = self.read_buffer[..self.seen_bytes].into();
        self.finish_message();
        discarded_bytes
    }

    pub fn progress(&mut self) -> Result<ReceiveEvent<C>, FlowInterrupt<ReceiveError<C>>>
    where
        C: Decoder,
        for<'a> C::Message<'a>: IntoBoundedStatic<Static = C::Message<'static>>,
        for<'a> C::Error<'a>: IntoBoundedStatic<Static = C::Error<'static>>,
    {
        let event = loop {
            match self.next_fragment {
                NextFragment::Line { seen_bytes_in_line } => {
                    break self.progress_line(seen_bytes_in_line)?;
                }
                NextFragment::Literal { length } => {
                    self.progress_literal(length)?;
                }
            };
        };

        Ok(event)
    }

    fn progress_line(
        &mut self,
        seen_bytes_in_line: usize,
    ) -> Result<ReceiveEvent<C>, FlowInterrupt<ReceiveError<C>>>
    where
        C: Decoder,
        for<'a> C::Message<'a>: IntoBoundedStatic<Static = C::Message<'static>>,
        for<'a> C::Error<'a>: IntoBoundedStatic<Static = C::Error<'static>>,
    {
        let max_readable_bytes = self.max_readable_bytes();

        let Some(crlf_result) = find_crlf(
            &self.read_buffer[self.seen_bytes..max_readable_bytes],
            seen_bytes_in_line,
            self.crlf_relaxed,
        ) else {
            // No full line received yet, more data needed.

            // Mark the bytes of the partial line as seen.
            let seen_bytes_in_line = self.read_buffer.len() - self.seen_bytes;
            self.next_fragment = NextFragment::Line { seen_bytes_in_line };

            // Abort if we can't request more data.
            if Some(max_readable_bytes) == self.max_message_size.map(|size| size as usize) {
                self.seen_bytes = max_readable_bytes;
                return Err(FlowInterrupt::Error(ReceiveError::MessageTooLong));
            }

            // Request more data.
            return Err(FlowInterrupt::Io(FlowIo::NeedMoreInput));
        };

        // Mark the all bytes of the current line as seen.
        self.seen_bytes += crlf_result.lf_position + 1;
        self.next_fragment = NextFragment::start_new_line();

        if crlf_result.expected_crlf_got_lf {
            return Err(FlowInterrupt::Error(ReceiveError::ExpectedCrlfGotLf));
        }

        // Try to parse the whole message from the start (including the new line).
        // TODO(#129): If the message is really long and we need multiple attempts to receive it,
        //             then this is O(n^2). IMO this can be only fixed by using a generator-like
        //             decoder.
        match self.codec.decode(&self.read_buffer[..self.seen_bytes]) {
            Ok((remaining, message)) => {
                assert!(remaining.is_empty());
                Ok(ReceiveEvent::DecodingSuccess(message.into_static()))
            }
            Err(error) => Err(FlowInterrupt::Error(ReceiveError::DecodingFailure(
                error.into_static(),
            ))),
        }
    }

    fn progress_literal(
        &mut self,
        literal_length: u32,
    ) -> Result<(), FlowInterrupt<ReceiveError<C>>>
    where
        C: Decoder,
    {
        let max_readable_bytes = self.max_readable_bytes();
        let unseen_bytes = max_readable_bytes - self.seen_bytes;

        if unseen_bytes < literal_length as usize {
            // We did not receive enough bytes for the literal yet.

            // Abort if we can't request more data.
            if Some(max_readable_bytes) == self.max_message_size.map(|size| size as usize) {
                self.seen_bytes = max_readable_bytes;
                return Err(FlowInterrupt::Error(ReceiveError::MessageTooLong));
            }

            // Request more data.
            return Err(FlowInterrupt::Io(FlowIo::NeedMoreInput));
        } else {
            // We received enough bytes for the literal.
            // Now we can continue reading the next line.
            self.next_fragment = NextFragment::start_new_line();
            self.seen_bytes += literal_length as usize;
        }

        Ok(())
    }

    fn max_readable_bytes(&self) -> usize {
        let readable_bytes = self.read_buffer.len();
        self.max_message_size
            .map_or(readable_bytes, |size| readable_bytes.min(size as usize))
    }

    pub fn change_codec<D>(self, codec: D) -> ReceiveState<D> {
        ReceiveState::with_read_buffer(
            codec,
            self.crlf_relaxed,
            self.max_message_size,
            self.read_buffer,
        )
    }
}

pub enum ReceiveEvent<C: Decoder> {
    DecodingSuccess(C::Message<'static>),
}

pub enum ReceiveError<C: Decoder> {
    DecodingFailure(C::Error<'static>),
    ExpectedCrlfGotLf,
    MessageTooLong,
}

/// The next fragment that will be read...
#[derive(Clone, Copy, Debug)]
enum NextFragment {
    // ... is a line.
    //
    // Note: A message always starts (and ends) with a line.
    Line {
        // How many bytes in the current line do we already have checked?
        // This is important if we need multiple attempts to read from the underlying
        // stream before the line is completely received.
        seen_bytes_in_line: usize,
    },
    // ... is a literal with the given length.
    Literal {
        length: u32,
    },
}

impl NextFragment {
    fn start_new_line() -> Self {
        Self::Line {
            seen_bytes_in_line: 0,
        }
    }
}

/// A line ending for the current line was found.
struct FindCrlfResult {
    // The position of the `\n` symbol
    lf_position: usize,
    // Is the line ending `\n` even though we expected `\r\n`?
    expected_crlf_got_lf: bool,
}

/// Finds the line ending (`\n` or `\r\n`) for the current line.
///
/// Parameters:
/// - `buf`: The buffer that contains the current line starting at index 0.
/// - `start`: At this index the search for `\n` will start. Note that the `\r` might be located
//     before this index.
/// - `crlf_relaxed`: Whether the accepted line ending is `\n` or `\r\n`.
fn find_crlf(buf: &[u8], start: usize, crlf_relaxed: bool) -> Option<FindCrlfResult> {
    let lf_position = start + buf[start..].iter().position(|item| *item == b'\n')?;
    let expected_crlf_got_lf = !crlf_relaxed && buf[lf_position.saturating_sub(1)] != b'\r';
    Some(FindCrlfResult {
        lf_position,
        expected_crlf_got_lf,
    })
}
