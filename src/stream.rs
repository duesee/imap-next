use std::{
    collections::VecDeque,
    convert::Infallible,
    future::poll_fn,
    io::IoSlice,
    task::{ready, Context, Poll},
};

use futures_util::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    FutureExt,
};
#[cfg(debug_assertions)]
use imap_codec::imap_types::utils::escape_byte_string;
use thiserror::Error;
#[cfg(debug_assertions)]
use tracing::trace;

use crate::{Interrupt, Io, State};

pub struct Stream<S> {
    stream: S,
    read_buffer: ReadBuffer,
    write_buffer: WriteBuffer,
}

impl<S> Stream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            read_buffer: ReadBuffer::new(),
            write_buffer: WriteBuffer::new(),
        }
    }
}

impl<S> Stream<S> {
    #[cfg(feature = "expose_stream")]
    /// Return the underlying stream for debug purposes (or experiments).
    ///
    /// Note: Writing to or reading from the stream may introduce
    /// conflicts with `imap-next`.
    pub fn stream_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Take the underlying stream out of a [`Stream`].
    ///
    /// Useful when a TCP stream needs to be upgraded to a TLS one.
    #[cfg(feature = "expose_stream")]
    pub fn into_stream(self) -> S {
        self.stream
    }
}

impl<S: AsyncWrite + Unpin> Stream<S> {
    pub async fn flush(&mut self) -> Result<(), Error<Infallible>> {
        poll_fn(|cx| self.write_buffer.poll_write(&mut self.stream, cx)).await?;
        self.stream.flush().await?;

        Ok(())
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Stream<S> {
    pub async fn next<F: State>(&mut self, mut state: F) -> Result<F::Event, Error<F::Error>> {
        let event = loop {
            // Progress the client/server
            let result = state.next();

            // Return events immediately without doing IO
            let interrupt = match result {
                Err(interrupt) => interrupt,
                Ok(event) => break event,
            };

            // Return errors immediately without doing IO
            let io = match interrupt {
                Interrupt::Io(io) => io,
                Interrupt::Error(err) => return Err(Error::State(err)),
            };

            // Handle the output bytes from the client/server
            if let Io::Output(bytes) = io {
                self.write_buffer.push_bytes(bytes);
            }

            // Progress the stream
            poll_fn(|cx| {
                let bytes = if self.write_buffer.needs_write() {
                    // We read and write the stream simultaneously because otherwise
                    // a deadlock between client and server might occur if both sides
                    // would only read or only write. We achieve this by polling both
                    // operations before blocking.
                    match self.write_buffer.poll_write(&mut self.stream, cx) {
                        Poll::Ready(result) => return Poll::Ready(result),
                        Poll::Pending => {
                            ready!(self.read_buffer.poll_read(&mut self.stream, cx)?)
                        }
                    }
                } else {
                    // Nothing to write, just read
                    ready!(self.read_buffer.poll_read(&mut self.stream, cx)?)
                };

                // Provide input bytes to the client/server and try again
                state.enqueue_input(bytes);
                Poll::Ready(Ok(()))
            })
            .await?;
        };

        Ok(event)
    }
}

/// Error during reading into or writing from a stream.
#[derive(Debug, Error)]
pub enum Error<E> {
    /// Operation failed because stream is closed.
    ///
    /// We detect this by checking if the read or written byte count is 0. Whether the stream is
    /// closed indefinitely or temporarily depends on the actual stream implementation.
    #[error("Stream was closed")]
    Closed,
    /// An I/O error occurred in the underlying stream.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// An error occurred while progressing the state.
    #[error(transparent)]
    State(E),
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

    fn poll_read<S: AsyncRead + Unpin>(
        &mut self,
        stream: &mut S,
        cx: &mut Context<'_>,
    ) -> Poll<Result<&[u8], ReadWriteError>> {
        // Constructing this future is cheap
        let byte_count = ready!(stream.read(&mut self.bytes).poll_unpin(cx)?);

        #[cfg(debug_assertions)]
        trace!(
            data = escape_byte_string(&self.bytes[0..byte_count]),
            "io/read/raw"
        );

        if byte_count == 0 {
            // The result is 0 if the stream reached "end of file"
            return Poll::Ready(Err(ReadWriteError::Closed));
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

    fn push_bytes(&mut self, bytes: Vec<u8>) {
        self.bytes.extend(bytes)
    }

    fn needs_write(&self) -> bool {
        !self.bytes.is_empty()
    }

    fn write_slices(&mut self) -> [IoSlice; 2] {
        let (init, tail) = self.bytes.as_slices();
        [IoSlice::new(init), IoSlice::new(tail)]
    }

    fn poll_write<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), ReadWriteError>> {
        while self.needs_write() {
            let write_slices = &self.write_slices();

            // Constructing this future is cheap
            let byte_count = ready!(stream.write_vectored(write_slices).poll_unpin(cx)?);

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
                return Poll::Ready(Err(ReadWriteError::Closed));
            }
        }

        Poll::Ready(Ok(()))
    }
}

#[derive(Debug, Error)]
enum ReadWriteError {
    #[error("Stream was closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl<E> From<ReadWriteError> for Error<E> {
    fn from(value: ReadWriteError) -> Self {
        match value {
            ReadWriteError::Closed => Error::Closed,
            ReadWriteError::Io(err) => Error::Io(err),
        }
    }
}
