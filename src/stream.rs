use std::{fmt::Debug, pin::Pin};

use tokio::io::{AsyncRead, AsyncWrite};

// TODO: Reconsider this. Do we really need Stream + AnyStream? What is the smallest API that we need to expose?

pub trait Stream: AsyncRead + AsyncWrite + Send + Debug {}

impl<S: AsyncRead + AsyncWrite + Send + Debug> Stream for S {}

#[derive(Debug)]
pub struct AnyStream(pub Pin<Box<dyn Stream>>);

impl AnyStream {
    pub fn new<S: Stream + 'static>(stream: S) -> Self {
        Self(Box::pin(stream))
    }
}
