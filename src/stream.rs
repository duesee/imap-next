use std::{num::NonZeroUsize, pin::Pin};

use bytes::{Buf, BytesMut};
#[cfg(debug_assertions)]
use imap_types::utils::escape_byte_string;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(debug_assertions)]
use tracing::trace;

// TODO: Reconsider this. Do we really need Stream + AnyStream? What is the smallest API that we need to expose?

pub trait Stream: AsyncRead + AsyncWrite + Send {}

impl<S: AsyncRead + AsyncWrite + Send> Stream for S {}

pub struct AnyStream(pub Pin<Box<dyn Stream>>);

impl AnyStream {
    pub fn new<S: Stream + 'static>(stream: S) -> Self {
        Self(Box::pin(stream))
    }

    /// Reads at least one byte into the buffer and returns the number of read bytes.
    ///
    /// Returns [`StreamError::Closed`] when no bytes could be read.
    pub async fn read(&mut self, read_buffer: &mut BytesMut) -> Result<NonZeroUsize, StreamError> {
        #[cfg(debug_assertions)]
        let old_len = read_buffer.len();
        let byte_count = self.0.read_buf(read_buffer).await?;

        #[cfg(debug_assertions)]
        trace!(
            data = escape_byte_string(&read_buffer[old_len..]),
            "io/read/raw"
        );

        match NonZeroUsize::new(byte_count) {
            None => {
                // The result is 0 if the stream reached "end of file" or the read buffer was
                // already full before calling `read_buf`. Because we use an unlimited buffer we
                // know that the first case occurred.
                Err(StreamError::Closed)
            }
            Some(byte_count) => Ok(byte_count),
        }
    }

    /// Writes all bytes from the write buffer.
    ///
    /// Returns [`StreamError::Closed`] when not all bytes could be written.
    pub async fn write_all(&mut self, write_buffer: &mut BytesMut) -> Result<(), StreamError> {
        while !write_buffer.is_empty() {
            let byte_count = self.0.write(write_buffer).await?;
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
                return Err(StreamError::Closed);
            }
        }

        Ok(())
    }
}

/// Error during reading from or writing to a [`Stream`].
#[derive(Debug, Error)]
pub enum StreamError {
    /// The operation failed because the stream is closed.
    ///
    /// We detect this by checking if the read or written byte count is 0. Whether the stream is
    /// closed indefinitely or temporarily depend on the actual stream implementation.
    #[error("Stream was closed")]
    Closed,
    /// An I/O error occurred in the underlying stream.
    #[error(transparent)]
    Io(#[from] tokio::io::Error),
}
