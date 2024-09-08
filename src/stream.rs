use std::{
    convert::Infallible,
    future::{poll_fn, Future},
    pin::pin,
    task::{Context, Poll},
};

use bytes::{Buf, BytesMut};
#[cfg(debug_assertions)]
use imap_codec::imap_types::utils::escape_byte_string;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(debug_assertions)]
use tracing::trace;

use crate::{Interrupt, Io, State};

pub struct Stream<S> {
    stream: S,
    read_buffer: BytesMut,
    write_buffer: BytesMut,
}

impl<S> Stream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            read_buffer: BytesMut::default(),
            write_buffer: BytesMut::default(),
        }
    }
}

impl<S: Unpin> Stream<S> {
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
        poll_fn(|cx| poll_write_stream(&mut self.stream, cx, &mut self.write_buffer)).await?;
        self.stream.flush().await?;

        Ok(())
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Stream<S> {
    pub async fn next<F: State>(&mut self, mut state: F) -> Result<F::Event, Error<F::Error>> {
        let event = loop {
            // Provide input bytes to the client/server
            if !self.read_buffer.is_empty() {
                state.enqueue_input(&self.read_buffer);
                self.read_buffer.clear();
            }

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
                self.write_buffer.extend(bytes);
            }

            // Progress the stream
            if self.write_buffer.is_empty() {
                poll_fn(|cx| poll_read_stream(&mut self.stream, cx, &mut self.read_buffer)).await?;
            } else {
                // We read and write the stream simultaneously because otherwise
                // a deadlock between client and server might occur if both sides
                // would only read or only write. We achieve this by polling both
                // operations before blocking.
                poll_fn(|cx| {
                    match poll_write_stream(&mut self.stream, cx, &mut self.write_buffer) {
                        Poll::Ready(result) => Poll::Ready(result),
                        Poll::Pending => {
                            poll_read_stream(&mut self.stream, cx, &mut self.read_buffer)
                        }
                    }
                })
                .await?;
            };
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
    Io(#[from] tokio::io::Error),
    /// An error occurred while progressing the state.
    #[error(transparent)]
    State(E),
}

fn poll_read_stream<S: AsyncRead + Unpin>(
    stream: &mut S,
    cx: &mut Context<'_>,
    read_buffer: &mut BytesMut,
) -> Poll<Result<(), ReadWriteError>> {
    #[cfg(debug_assertions)]
    let old_len = read_buffer.len();

    // Constructing this future is cheap
    let read_buf_future = pin!(stream.read_buf(read_buffer));
    let Poll::Ready(read_buf_result) = read_buf_future.poll(cx) else {
        return Poll::Pending;
    };
    let byte_count = read_buf_result?;

    #[cfg(debug_assertions)]
    trace!(
        data = escape_byte_string(&read_buffer[old_len..]),
        "io/read/raw"
    );

    if byte_count == 0 {
        // The result is 0 if the stream reached "end of file" or the read buffer was
        // already full before calling `read_buf`. Because we use an unlimited buffer we
        // know that the first case occurred.
        return Poll::Ready(Err(ReadWriteError::Closed));
    }

    Poll::Ready(Ok(()))
}

fn poll_write_stream<S: AsyncWrite + Unpin>(
    stream: &mut S,
    cx: &mut Context<'_>,
    write_buffer: &mut BytesMut,
) -> Poll<Result<(), ReadWriteError>> {
    while !write_buffer.is_empty() {
        // Constructing this future is cheap
        let write_future = pin!(stream.write(write_buffer));
        let Poll::Ready(write_result) = write_future.poll(cx) else {
            return Poll::Pending;
        };
        let byte_count = write_result?;

        #[cfg(debug_assertions)]
        trace!(
            data = escape_byte_string(&write_buffer[..byte_count]),
            "io/write/raw"
        );

        write_buffer.advance(byte_count);

        if byte_count == 0 {
            // The result is 0 if the stream doesn't accept bytes anymore or the write buffer
            // was already empty before calling `write_buf`. Because we checked the buffer
            // we know that the first case occurred.
            return Poll::Ready(Err(ReadWriteError::Closed));
        }
    }

    Poll::Ready(Ok(()))
}

#[derive(Debug, Error)]
enum ReadWriteError {
    #[error("Stream was closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] tokio::io::Error),
}

impl<E> From<ReadWriteError> for Error<E> {
    fn from(value: ReadWriteError) -> Self {
        match value {
            ReadWriteError::Closed => Error::Closed,
            ReadWriteError::Io(err) => Error::Io(err),
        }
    }
}
