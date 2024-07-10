use std::net::SocketAddr;

use bstr::ByteSlice;
use imap_next::{
    client::{self, Client, CommandHandle},
    stream::{self, Stream},
};
use imap_types::{command::Command, ToStatic};
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
        client_options: client::Options,
        server_address: SocketAddr,
    ) -> Self {
        let stream = TcpStream::connect(server_address).await.unwrap();
        trace!(?server_address, "Client is connected");
        let stream = Stream::insecure(stream);
        let client = Client::new(client_options);
        Self {
            codecs,
            connection_state: ConnectionState::Connected { stream, client },
        }
    }

    pub async fn receive_greeting(&mut self, expected_bytes: &[u8]) {
        let expected_greeting = self.codecs.decode_greeting(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::GreetingReceived { greeting } => {
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

    pub fn set_idle_done(&mut self, idle_handle: CommandHandle) {
        let (_, client) = self.connection_state.connected();
        let Some(handle) = client.set_idle_done() else {
            panic!("Client is in unexpected state");
        };
        assert_eq!(idle_handle, handle);
    }

    pub fn set_authenticate_data(&mut self, authenticate_handle: CommandHandle, bytes: &[u8]) {
        let authenticate_data = self
            .codecs
            .decode_authenticate_data_normalized(bytes)
            .to_static();
        let (_, client) = self.connection_state.connected();
        let Ok(handle) = client.set_authenticate_data(authenticate_data.to_static()) else {
            panic!("Client is in unexpected state");
        };
        assert_eq!(authenticate_handle, handle);
    }

    pub async fn progress_command(&mut self, enqueued_command: EnqueuedCommand) {
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::CommandSent { handle, command } => {
                assert_eq!(enqueued_command.handle, handle);
                assert_eq!(enqueued_command.command, command);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn progress_idle(&mut self, idle_handle: CommandHandle) {
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::IdleCommandSent { handle } => {
                assert_eq!(idle_handle, handle);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn progress_idle_done(&mut self, idle_handle: CommandHandle) {
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::IdleDoneSent { handle } => {
                assert_eq!(idle_handle, handle);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn progress_authenticate(&mut self, authenticate_handle: CommandHandle) {
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::AuthenticateStarted { handle } => {
                assert_eq!(authenticate_handle, handle);
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
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::CommandRejected {
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

    /// Progresses internal commands without expecting any results.
    pub async fn progress_internal_commands<T>(&mut self) -> T {
        let (stream, client) = self.connection_state.connected();
        let result = stream.next(client).await;
        panic!("Client has unexpected result: {result:?}");
    }

    pub async fn send_command(&mut self, bytes: &[u8]) {
        let enqueued_command = self.enqueue_command(bytes);
        self.progress_command(enqueued_command).await;
    }

    pub async fn send_idle(&mut self, bytes: &[u8]) -> CommandHandle {
        let enqueued_command = self.enqueue_command(bytes);
        self.progress_idle(enqueued_command.handle).await;
        enqueued_command.handle
    }

    pub async fn send_idle_done(&mut self, idle_handle: CommandHandle) {
        self.set_idle_done(idle_handle);
        self.progress_idle_done(idle_handle).await;
    }

    pub async fn send_authenticate(&mut self, bytes: &[u8]) -> CommandHandle {
        let enqueued_command = self.enqueue_command(bytes);
        self.progress_authenticate(enqueued_command.handle).await;
        enqueued_command.handle
    }

    pub async fn send_rejected_command(&mut self, command_bytes: &[u8], status_bytes: &[u8]) {
        let enqueued_command = self.enqueue_command(command_bytes);
        self.progress_rejected_command(enqueued_command, status_bytes)
            .await;
    }

    pub async fn receive_data(&mut self, expected_bytes: &[u8]) {
        let expected_data = self.codecs.decode_data(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::DataReceived { data } => {
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
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::StatusReceived { status } => {
                assert_eq!(expected_status, status);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_idle_accepted(
        &mut self,
        idle_handle: CommandHandle,
        expected_bytes: &[u8],
    ) {
        let expected_continuation_request = self.codecs.decode_continuation_request(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::IdleAccepted {
                handle,
                continuation_request,
            } => {
                assert_eq!(handle, idle_handle);
                assert_eq!(expected_continuation_request, continuation_request);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_idle_rejected(
        &mut self,
        idle_handle: CommandHandle,
        expected_bytes: &[u8],
    ) {
        let expected_status = self.codecs.decode_status(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::IdleRejected { handle, status } => {
                assert_eq!(handle, idle_handle);
                assert_eq!(status, expected_status);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_authenticate_continuation_request(
        &mut self,
        authenticate_request: CommandHandle,
        expected_bytes: &[u8],
    ) {
        let expected_continuation_request = self.codecs.decode_continuation_request(expected_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::AuthenticateContinuationRequestReceived {
                handle,
                continuation_request,
            } => {
                assert_eq!(handle, authenticate_request);
                assert_eq!(expected_continuation_request, continuation_request);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_authenticate_status(
        &mut self,
        authenticate_request: CommandHandle,
        expected_authenticate_bytes: &[u8],
        expected_status_bytes: &[u8],
    ) {
        let expected_command = self
            .codecs
            .decode_command_normalized(expected_authenticate_bytes);
        let expected_status = self.codecs.decode_status_normalized(expected_status_bytes);
        let (stream, client) = self.connection_state.connected();
        let event = stream.next(client).await.unwrap();
        match event {
            client::Event::AuthenticateStatusReceived {
                handle,
                command_authenticate,
                status,
            } => {
                assert_eq!(handle, authenticate_request);
                assert_eq!(expected_command, command_authenticate.into());
                assert_eq!(expected_status, status);
            }
            event => {
                panic!("Client emitted unexpected event: {event:?}");
            }
        }
    }

    async fn receive_error(&mut self) -> client::Error {
        let error = match &mut self.connection_state {
            ConnectionState::Connected { stream, client } => stream.next(client).await.unwrap_err(),
            ConnectionState::Disconnected => {
                panic!("Client is already disconnected")
            }
        };

        match error {
            stream::Error::State(err) => err,
            err => {
                panic!("Client emitted unexpected error: {err:?}");
            }
        }
    }

    pub async fn receive_error_because_expected_crlf_got_lf(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            client::Error::ExpectedCrlfGotLf { discarded_bytes } => {
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
            client::Error::MalformedMessage { discarded_bytes } => {
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
    Connected { stream: Stream, client: Client },
    /// Connection dropped.
    Disconnected,
}

impl ConnectionState {
    fn connected(&mut self) -> (&mut Stream, &mut Client) {
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
    handle: CommandHandle,
    command: Command<'static>,
}
