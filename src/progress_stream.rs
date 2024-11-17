use std::{
    collections::VecDeque,
    io::IoSlice,
    pin::{pin, Pin},
    task::{ready, Context, Poll},
};

use futures_io::{AsyncRead, AsyncWrite};
#[cfg(debug_assertions)]
use imap_codec::imap_types::utils::escape_byte_string;
use thiserror::Error;
#[cfg(debug_assertions)]
use tracing::trace;

pub struct ProgressStream<S> {
    stream: S,
    read_buffer: ReadBuffer,
    write_buffer: WriteBuffer,
}

impl<S> ProgressStream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            read_buffer: ReadBuffer::new(),
            write_buffer: WriteBuffer::new(),
        }
    }
}

impl<S> ProgressStream<S> {
    #[cfg(feature = "expose_stream")]
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    #[cfg(feature = "expose_stream")]
    pub fn into_inner(self) -> S {
        self.stream
    }

    pub fn enqueue_bytes(&mut self, bytes: &[u8]) {
        self.write_buffer.push_bytes(bytes)
    }
}

impl<S: AsyncWrite + Unpin> ProgressStream<S> {
    pub fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), ProgressError>> {
        let mut stream = pin!(&mut self.stream);
        ready!(self.write_buffer.poll_write(cx, stream.as_mut()))?;
        ready!(stream.poll_flush(cx))?;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> ProgressStream<S> {
    pub fn poll_progress<'a>(
        &'a mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<&'a [u8], ProgressError>> {
        // We read and write the stream simultaneously because otherwise
        // a deadlock between client and server might occur if both sides
        // would only read or only write. We achieve this by polling both
        // operations.
        let mut stream = pin!(&mut self.stream);
        let _ = self.write_buffer.poll_write(cx, stream.as_mut())?;
        self.read_buffer.poll_read(cx, stream)
    }
}

struct ReadBuffer {
    /// Temporary read buffer for input bytes.
    ///
    /// The buffer is non-empty and and has a fixed-size. Read bytes will overwrite the
    /// buffer and then immediately enqueued as input to [`State`].
    bytes: Box<[u8]>,
}

impl ReadBuffer {
    fn new() -> Self {
        Self {
            bytes: vec![0; 1024].into(),
        }
    }

    fn poll_read<S: AsyncRead>(
        &mut self,
        cx: &mut Context<'_>,
        stream: Pin<&mut S>,
    ) -> Poll<Result<&[u8], ProgressError>> {
        // Constructing this future is cheap
        let byte_count = ready!(stream.poll_read(cx, &mut self.bytes))?;

        #[cfg(debug_assertions)]
        trace!(
            data = escape_byte_string(&self.bytes[0..byte_count]),
            "io/read/raw"
        );

        if byte_count == 0 {
            // The result is 0 if the stream reached "end of file"
            return Poll::Ready(Err(ProgressError::Closed));
        }

        Poll::Ready(Ok(&self.bytes[0..byte_count]))
    }
}

struct WriteBuffer {
    /// Queue for output bytes that needs to be written.
    ///
    /// The output of [`State`] will be written to this queue. Output bytes will be enqueued
    /// to the back and written bytes will be dequeued from the front.
    bytes: VecDeque<u8>,
}

impl WriteBuffer {
    fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
        }
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes)
    }

    fn needs_write(&self) -> bool {
        !self.bytes.is_empty()
    }

    fn write_slices(&mut self) -> [IoSlice; 2] {
        let (init, tail) = self.bytes.as_slices();
        [IoSlice::new(init), IoSlice::new(tail)]
    }

    fn poll_write<S: AsyncWrite>(
        &mut self,
        cx: &mut Context<'_>,
        mut stream: Pin<&mut S>,
    ) -> Poll<Result<(), ProgressError>> {
        while self.needs_write() {
            let write_slices = &self.write_slices();

            let byte_count = ready!(stream.as_mut().poll_write_vectored(cx, write_slices))?;

            #[cfg(debug_assertions)]
            trace!(
                data = escape_byte_string(
                    self.bytes
                        .iter()
                        .copied()
                        .take(byte_count)
                        .collect::<Vec<_>>()
                ),
                "io/write/raw"
            );

            // Drop written bytes
            drop(self.bytes.drain(..byte_count));

            if byte_count == 0 {
                // The result is 0 if the stream doesn't accept bytes anymore or the write buffer
                // was already empty before calling `write_buf`. Because we checked the buffer
                // we know that the first case occurred.
                return Poll::Ready(Err(ProgressError::Closed));
            }
        }

        Poll::Ready(Ok(()))
    }
}

#[derive(Debug, Error)]
pub enum ProgressError {
    #[error("Stream was closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
