use bstr::ByteSlice;
use imap_flow::{
    server::{ServerFlow, ServerFlowError, ServerFlowEvent, ServerFlowOptions},
    stream::{Stream, StreamError},
};
use imap_types::{bounded_static::ToBoundedStatic, response::Response};
use tokio::net::TcpListener;
use tracing::trace;

use crate::codecs::Codecs;

/// A wrapper for `ServerFlow` suitable for testing.
pub struct ServerTester {
    codecs: Codecs,
    server_flow_options: ServerFlowOptions,
    connection_state: ConnectionState,
}

impl ServerTester {
    pub async fn new(
        codecs: Codecs,
        server_flow_options: ServerFlowOptions,
        server_listener: TcpListener,
    ) -> Self {
        let (stream, client_address) = server_listener.accept().await.unwrap();
        trace!(?client_address, "Server accepts connection");
        let stream = Stream::insecure(stream);
        Self {
            codecs,
            server_flow_options,
            connection_state: ConnectionState::Connected { stream },
        }
    }

    pub async fn send_greeting(&mut self, bytes: &[u8]) {
        let enqueued_greeting = self.codecs.decode_greeting_normalized(bytes);
        match self.connection_state.take() {
            ConnectionState::Connected { mut stream } => {
                let mut server = ServerFlow::new(
                    self.server_flow_options.clone(),
                    enqueued_greeting.to_static(),
                );
                let event = stream.progress(&mut server).await.unwrap();
                match event {
                    ServerFlowEvent::GreetingSent { greeting } => {
                        assert_eq!(enqueued_greeting, greeting);
                    }
                    event => {
                        panic!("Server has unexpected event: {event:?}");
                    }
                }
                self.connection_state = ConnectionState::Greeted { stream, server };
            }
            ConnectionState::Greeted { .. } => {
                panic!("Server has already greeted");
            }
            ConnectionState::Disconnected => {
                panic!("Server is already disconnected");
            }
        }
    }

    pub async fn receive_command(&mut self, expected_bytes: &[u8]) {
        let expected_command = self.codecs.decode_command(expected_bytes);
        let (stream, server) = self.connection_state.greeted();
        let event = stream.progress(server).await.unwrap();
        match event {
            ServerFlowEvent::CommandReceived { command } => {
                assert_eq!(expected_command, command);
            }
            event => {
                panic!("Server emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn send_data(&mut self, bytes: &[u8]) {
        let enqueued_data = self.codecs.decode_data_normalized(bytes);
        let (stream, server) = self.connection_state.greeted();
        let enqueued_handle = server.enqueue_data(enqueued_data.to_static());
        let event = stream.progress(server).await.unwrap();
        match event {
            ServerFlowEvent::ResponseSent { handle, response } => {
                assert_eq!(enqueued_handle, handle);
                assert_eq!(Response::Data(enqueued_data), response);
            }
            event => {
                panic!("Server has unexpected event: {event:?}");
            }
        }
    }

    pub async fn send_status(&mut self, bytes: &[u8]) {
        let enqueued_status = self.codecs.decode_status_normalized(bytes);
        let (stream, server) = self.connection_state.greeted();
        let enqueued_handle = server.enqueue_status(enqueued_status.to_static());
        let event = stream.progress(server).await.unwrap();
        match event {
            ServerFlowEvent::ResponseSent { handle, response } => {
                assert_eq!(enqueued_handle, handle);
                assert_eq!(Response::Status(enqueued_status), response);
            }
            event => {
                panic!("Server has unexpected event: {event:?}");
            }
        }
    }

    async fn receive_error(&mut self) -> ServerFlowError {
        let (stream, server) = self.connection_state.greeted();
        let error = stream.progress(server).await.unwrap_err();
        match error {
            StreamError::Flow(err) => err,
            err => {
                panic!("Server emitted unexpected error: {err:?}");
            }
        }
    }

    pub async fn receive_error_because_expected_crlf_got_lf(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            ServerFlowError::ExpectedCrlfGotLf { discarded_bytes } => {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    discarded_bytes.declassify().as_bstr()
                );
            }
            error => {
                panic!("Server has unexpected error: {error:?}");
            }
        }
    }

    pub async fn receive_error_because_malformed_message(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            ServerFlowError::MalformedMessage { discarded_bytes } => {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    discarded_bytes.declassify().as_bstr()
                );
            }
            error => {
                panic!("Server has unexpected error: {error:?}");
            }
        }
    }

    pub async fn receive_error_because_literal_too_long(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            ServerFlowError::LiteralTooLong { discarded_bytes } => {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    discarded_bytes.declassify().as_bstr()
                );
            }
            error => {
                panic!("Server has unexpected error: {error:?}");
            }
        }
    }

    pub async fn receive_error_because_command_too_long(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            ServerFlowError::CommandTooLong { discarded_bytes } => {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    discarded_bytes.declassify().as_bstr()
                );
            }
            error => {
                panic!("Server has unexpected error: {error:?}");
            }
        }
    }

    /// Progresses internal responses without expecting any results.
    pub async fn progress_internal_responses<T>(&mut self) -> T {
        let (stream, server) = self.connection_state.greeted();
        let result = stream.progress(server).await;
        panic!("Server has unexpected result: {result:?}");
    }
}

/// The current state of the connection between server and client.
#[allow(clippy::large_enum_variant)]
enum ConnectionState {
    // The server has established a TCP connection to the client.
    Connected {
        stream: Stream<ServerFlow>,
    },
    // The server has greeted the client.
    Greeted {
        stream: Stream<ServerFlow>,
        server: ServerFlow,
    },
    // The TCP connection between server and client was dropped.
    Disconnected,
}

impl ConnectionState {
    fn greeted(&mut self) -> (&mut Stream<ServerFlow>, &mut ServerFlow) {
        match self {
            ConnectionState::Connected { .. } => {
                panic!("Server has not greeted yet");
            }
            ConnectionState::Greeted { stream, server } => (stream, server),
            ConnectionState::Disconnected => {
                panic!("Server is already disconnected");
            }
        }
    }

    fn take(&mut self) -> ConnectionState {
        std::mem::replace(self, ConnectionState::Disconnected)
    }
}
