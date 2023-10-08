use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

// TODO: Reconsider this. Do we really need Stream + AnyStream? What is the smallest API that we need to expose?

pub trait Stream: AsyncRead + AsyncWrite + Send {}

impl<S: AsyncRead + AsyncWrite + Send> Stream for S {}

pub struct AnyStream(pub Pin<Box<dyn Stream>>);

impl AnyStream {
    pub fn new<S: Stream + 'static>(stream: S) -> Self {
        Self(Box::pin(stream))
    }
}
