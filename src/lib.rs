#![forbid(unsafe_code)]

pub mod client;
mod handle;
mod receive;
mod send_command;
mod send_response;
pub mod server;
#[cfg(feature = "stream")]
pub mod stream;
#[cfg(test)]
mod tests;
pub mod types;

/// Simple abstraction of IMAP flows that can be used for implementing IO utilities.
///
/// The utility [`Stream`](stream::Stream) is using this for implementing the IO bits when
/// dealing with TCP or TLS.
pub trait Flow {
    type Event;
    type Error;

    fn enqueue_input(&mut self, bytes: &[u8]);
    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>>;
}

/// The IMAP flow was interrupted by an event that needs to be handled externally.
#[must_use = "If the IMAP flow is interrupted the interrupt must be handled. Ignoring this might result in a deadlock on IMAP level"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowInterrupt<E> {
    /// An IO operation is necessary. Ignoring this might result in a deadlock on IMAP level.
    Io(FlowIo),
    /// An error occurred. Ignoring this might result in an undefined IMAP state.
    Error(E),
}

/// The user of `imap-flow` must perform an IO operation in order to progress the IMAP flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowIo {
    /// More bytes must be read and passed to [`Flow::enqueue_input`].
    NeedMoreInput,
    /// The given bytes must be written.
    Output(Vec<u8>),
}
