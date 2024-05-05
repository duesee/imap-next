#![forbid(unsafe_code)]

pub mod client;
mod client_receive;
mod client_send;
mod handle;
mod receive;
pub mod server;
mod server_receive;
mod server_send;
#[cfg(feature = "stream")]
pub mod stream;
#[cfg(test)]
mod tests;
pub mod types;

// Re-expose codec
pub use imap_codec;

/// Common sans I/O interface used to implement I/O drivers for types that implement our protocol flows.
///
/// Most notably, [`ClientFlow`](client::ClientFlow) and [`ServerFlow`](server::ServerFlow) both implement
/// this trait whereas [`Stream`](stream::Stream) implements the I/O drivers.
pub trait Flow {
    /// Event emitted while progressing the protocol flow.
    type Event;

    /// Error emitted while progressing the protocol flow.
    type Error;

    /// Enqueue input bytes.
    ///
    /// These bytes may be used during the next [`Self::progress`] call.
    fn enqueue_input(&mut self, bytes: &[u8]);

    /// Progress the protocol flow until the next event (or interrupt).
    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>>;
}

impl<F: Flow> Flow for &mut F {
    type Event = F::Event;
    type Error = F::Error;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        (*self).enqueue_input(bytes);
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        (*self).progress()
    }
}

/// Protocol flow was interrupted by an event that needs to be handled externally.
#[must_use = "If the protocol flow is interrupted the interrupt must be handled. Ignoring this might result in a deadlock on IMAP level"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowInterrupt<E> {
    /// An IO operation is necessary. Ignoring this might result in a deadlock on IMAP level.
    Io(FlowIo),
    /// An error occurred. Ignoring this might result in an undefined IMAP state.
    Error(E),
}

/// User of `imap-flow` must perform an IO operation to progress the protocol flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowIo {
    /// More bytes must be read and passed to [`Flow::enqueue_input`].
    NeedMoreInput,
    /// Given bytes must be written.
    Output(Vec<u8>),
}
