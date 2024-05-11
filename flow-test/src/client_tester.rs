use std::net::SocketAddr;

use bstr::ByteSlice;
use imap_flow::{
    client::{
        ClientFlow, ClientFlowCommandHandle, ClientFlowError, ClientFlowEvent, ClientFlowOptions,
    },
    stream::{Stream, StreamError},
};
use imap_types::{bounded_static::ToBoundedStatic, command::Command};
use tokio::net::TcpStream;
use tracing::trace;

use crate::codecs::Codecs;

/// A wrapper for `ClientFlow` suitable for testing.
pub struct ClientTester {
    codecs: Codecs,
    connection_state: ConnectionState,
}

impl ClientTester {
    pub async fn new(
        codecs: Codecs,
        client_flow_options: ClientFlowOptions,
        server_address: SocketAddr,
    ) -> Self {
        let stream = TcpStream::connect(server_address).await.unwrap();
        trace!(?server_address, "Client is connected");
        let stream = Stream::insecure(stream);
        let client = ClientFlow::new(client_flow_options);
        Self {
            codecs,
            connection_state: ConnectionState::Connected { stream, client },
        }
    }

    pub async fn receive_greeting(&mut self, expected_bytes: &[u8]) {
        let expected_greeting = self.codecs.decode_greeting(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.progress(client).await.unwrap();
        match event {
            ClientFlowEvent::GreetingReceived { greeting } => {
                assert_eq!(expected_greeting, greeting);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub fn enqueue_command(&mut self, bytes: &[u8]) -> EnqueuedCommand {
        let command = self.codecs.decode_command_normalized(bytes).to_static();
        let (_, client) = self.connection_state.connected();
        let handle = client.enqueue_command(command.to_static());
        EnqueuedCommand { command, handle }
    }

    pub async fn progress_command(&mut self, enqueued_command: EnqueuedCommand) {
        let (stream, client) = self.connection_state.connected();
        let event = stream.progress(client).await.unwrap();
        match event {
            ClientFlowEvent::CommandSent { handle, command } => {
                assert_eq!(enqueued_command.handle, handle);
                assert_eq!(enqueued_command.command, command);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn progress_rejected_command(
        &mut self,
        enqueued_command: EnqueuedCommand,
        status_bytes: &[u8],
    ) {
        let expected_status = self.codecs.decode_status(status_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.progress(client).await.unwrap();
        match event {
            ClientFlowEvent::CommandRejected {
                handle,
                command,
                status,
            } => {
                assert_eq!(enqueued_command.handle, handle);
                assert_eq!(enqueued_command.command, command);
                assert_eq!(expected_status, status);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn send_command(&mut self, bytes: &[u8]) {
        let enqueued_command = self.enqueue_command(bytes);
        self.progress_command(enqueued_command).await;
    }

    pub async fn send_rejected_command(&mut self, command_bytes: &[u8], status_bytes: &[u8]) {
        let enqueued_command = self.enqueue_command(command_bytes);
        self.progress_rejected_command(enqueued_command, status_bytes)
            .await;
    }

    pub async fn receive_data(&mut self, expected_bytes: &[u8]) {
        let expected_data = self.codecs.decode_data(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.progress(client).await.unwrap();
        match event {
            ClientFlowEvent::DataReceived { data } => {
                assert_eq!(expected_data, data);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_status(&mut self, expected_bytes: &[u8]) {
        let expected_status = self.codecs.decode_status(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.progress(client).await.unwrap();
        match event {
            ClientFlowEvent::StatusReceived { status } => {
                assert_eq!(expected_status, status);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    async fn receive_error(&mut self) -> ClientFlowError {
        let error = match &mut self.connection_state {
            ConnectionState::Connected { stream, client } => {
                stream.progress(client).await.unwrap_err()
            }
            ConnectionState::Disconnected => {
                panic!("Client is already disconnected")
            }
        };

        match error {
            StreamError::Flow(err) => err,
            err => {
                panic!("Client emitted unexpected error: {err:?}");
            }
        }
    }

    pub async fn receive_error_because_expected_crlf_got_lf(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            ClientFlowError::ExpectedCrlfGotLf { discarded_bytes } => {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    discarded_bytes.declassify().as_bstr()
                );
            }
            error => {
                panic!("Client emitted unexpected error: {error:?}");
            }
        }
    }

    pub async fn receive_error_because_malformed_message(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            ClientFlowError::MalformedMessage { discarded_bytes } => {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    discarded_bytes.declassify().as_bstr()
                );
            }
            error => {
                panic!("Client emitted unexpected error: {error:?}");
            }
        }
    }
}

/// Connection state between client and server.
#[allow(clippy::large_enum_variant)]
enum ConnectionState {
    /// Connection to server established.
    Connected { stream: Stream, client: ClientFlow },
    /// Connection dropped.
    Disconnected,
}

impl ConnectionState {
    fn connected(&mut self) -> (&mut Stream, &mut ClientFlow) {
        match self {
            ConnectionState::Connected { stream, client } => (stream, client),
            ConnectionState::Disconnected => {
                panic!("Client is already disconnected");
            }
        }
    }

    #[allow(unused)]
    fn take(&mut self) -> ConnectionState {
        std::mem::replace(self, ConnectionState::Disconnected)
    }
}

/// Enqueued command that can be used for assertions.
pub struct EnqueuedCommand {
    handle: ClientFlowCommandHandle,
    command: Command<'static>,
}
