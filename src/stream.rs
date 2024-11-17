use std::{
    convert::Infallible,
    future::poll_fn,
    task::{ready, Poll},
};

use futures_io::{AsyncRead, AsyncWrite};
use thiserror::Error;

use crate::{
    progress_stream::{ProgressError, ProgressStream},
    Interrupt, Io, State,
};

pub struct Stream<S> {
    stream: ProgressStream<S>,
}

impl<S> Stream<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream: ProgressStream::new(stream),
        }
    }
}

impl<S> Stream<S> {
    /// Return the underlying stream for debug purposes (or experiments).
    ///
    /// Note: Writing to or reading from the stream may introduce
    /// conflicts with `imap-next`.
    #[cfg(feature = "expose_stream")]
    pub fn stream_mut(&mut self) -> &mut S {
        self.stream.get_mut()
    }

    /// Take the underlying stream out of a [`Stream`].
    ///
    /// Useful when a TCP stream needs to be upgraded to a TLS one.
    #[cfg(feature = "expose_stream")]
    pub fn into_stream(self) -> S {
        self.stream.into_inner()
    }
}

impl<S: AsyncWrite + Unpin> Stream<S> {
    pub async fn flush(&mut self) -> Result<(), Error<Infallible>> {
        poll_fn(|cx| self.stream.poll_flush(cx)).await?;
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

            // Handle IO
            match io {
                Io::Output(bytes) => {
                    // Handle the output bytes from the client/server
                    self.stream.enqueue_bytes(&bytes);
                }
                Io::NeedMoreInput => {
                    // We need to progress the stream
                    poll_fn(|cx| -> Poll<Result<(), Error<F::Error>>> {
                        let bytes = ready!(self.stream.poll_progress(cx))?;

                        // Provide input bytes to the client/server and try again
                        state.enqueue_input(bytes);

                        Poll::Ready(Ok(()))
                    })
                    .await?
                }
            }
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

impl<E> From<ProgressError> for Error<E> {
    fn from(value: ProgressError) -> Self {
        match value {
            ProgressError::Closed => Error::Closed,
            ProgressError::Io(err) => Error::Io(err),
        }
    }
}
