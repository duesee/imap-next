use bstr::ByteSlice;
use imap_next::{
    imap_types::{response::Response, ToStatic},
    server::{self, ResponseHandle, Server},
    stream::{self, Stream},
};
use tokio::net::TcpListener;
use tracing::trace;

use crate::codecs::Codecs;

/// Wrapper for `ServerFlow` suitable for testing.
pub struct ServerTester {
    codecs: Codecs,
    server_options: server::Options,
    connection_state: ConnectionState,
}

impl ServerTester {
    pub async fn new(
        codecs: Codecs,
        server_options: server::Options,
        server_listener: TcpListener,
    ) -> Self {
        let (stream, client_address) = server_listener.accept().await.unwrap();
        trace!(?client_address, "Server accepts connection");
        let stream = Stream::insecure(stream);
        Self {
            codecs,
            server_options,
            connection_state: ConnectionState::Connected { stream },
        }
    }

    pub async fn send_greeting(&mut self, bytes: &[u8]) {
        let enqueued_greeting = self.codecs.decode_greeting_normalized(bytes);
        match self.connection_state.take() {
            ConnectionState::Connected { mut stream } => {
                let mut server =
                    Server::new(self.server_options.clone(), enqueued_greeting.to_static());
                let event = stream.next(&mut server).await.unwrap();
                match event {
                    server::Event::GreetingSent { greeting } => {
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

    pub fn enqueue_data(&mut self, bytes: &[u8]) -> EnqueuedResponse {
        let data = self.codecs.decode_data_normalized(bytes).to_static();
        let (_, server) = self.connection_state.greeted();
        let handle = server.enqueue_data(data.to_static());
        EnqueuedResponse {
            response: Response::Data(data),
            handle,
        }
    }

    pub fn enqueue_status(&mut self, bytes: &[u8]) -> EnqueuedResponse {
        let status = self.codecs.decode_status_normalized(bytes).to_static();
        let (_, server) = self.connection_state.greeted();
        let handle = server.enqueue_status(status.to_static());
        EnqueuedResponse {
            response: Response::Status(status),
            handle,
        }
    }

    pub fn set_idle_accept(&mut self, bytes: &[u8]) -> EnqueuedResponse {
        let continuation_request = self
            .codecs
            .decode_continuation_request_normalized(bytes)
            .to_static();
        let (_, server) = self.connection_state.greeted();
        let Ok(handle) = server.idle_accept(continuation_request.to_static()) else {
            panic!("Server is in unexpected state");
        };
        EnqueuedResponse {
            response: Response::CommandContinuationRequest(continuation_request),
            handle,
        }
    }

    pub fn set_idle_reject(&mut self, bytes: &[u8]) -> EnqueuedResponse {
        let status = self.codecs.decode_status_normalized(bytes).to_static();
        let (_, server) = self.connection_state.greeted();
        let Ok(handle) = server.idle_reject(status.to_static()) else {
            panic!("Server is in unexpected state");
        };
        EnqueuedResponse {
            response: Response::Status(status),
            handle,
        }
    }

    pub fn set_authenticate_continue(&mut self, bytes: &[u8]) -> EnqueuedResponse {
        let authenticate_data = self
            .codecs
            .decode_continuation_request_normalized(bytes)
            .to_static();
        let (_, server) = self.connection_state.greeted();
        let Ok(handle) = server.authenticate_continue(authenticate_data.to_static()) else {
            panic!("Server is in unexpected state");
        };
        EnqueuedResponse {
            response: Response::CommandContinuationRequest(authenticate_data),
            handle,
        }
    }

    pub fn set_authenticate_finish(&mut self, bytes: &[u8]) -> EnqueuedResponse {
        let authenticate_data = self.codecs.decode_status_normalized(bytes).to_static();
        let (_, server) = self.connection_state.greeted();
        let Ok(handle) = server.authenticate_finish(authenticate_data.to_static()) else {
            panic!("Server is in unexpected state");
        };
        EnqueuedResponse {
            response: Response::Status(authenticate_data),
            handle,
        }
    }

    pub async fn progress_response(&mut self, enqueued_response: EnqueuedResponse) {
        let (stream, server) = self.connection_state.greeted();
        let event = stream.next(server).await.unwrap();
        match event {
            server::Event::ResponseSent { handle, response } => {
                assert_eq!(enqueued_response.handle, handle);
                assert_eq!(enqueued_response.response, response);
            }
            event => {
                panic!("Server has unexpected event: {event:?}");
            }
        }
    }

    /// Progresses internal responses without expecting any results.
    pub async fn progress_internal_responses<T>(&mut self) -> T {
        let (stream, server) = self.connection_state.greeted();
        let result = stream.next(server).await;
        panic!("Server has unexpected result: {result:?}");
    }

    pub async fn send_data(&mut self, bytes: &[u8]) {
        let enqueued_response = self.enqueue_data(bytes);
        self.progress_response(enqueued_response).await;
    }

    pub async fn send_status(&mut self, bytes: &[u8]) {
        let enqueued_response = self.enqueue_status(bytes);
        self.progress_response(enqueued_response).await;
    }

    pub async fn send_idle_accepted(&mut self, bytes: &[u8]) {
        let enqueued_response = self.set_idle_accept(bytes);
        self.progress_response(enqueued_response).await;
    }

    pub async fn send_idle_rejected(&mut self, bytes: &[u8]) {
        let enqueued_response = self.set_idle_reject(bytes);
        self.progress_response(enqueued_response).await;
    }

    pub async fn send_authenticate_continue(&mut self, bytes: &[u8]) {
        let enqueued_response = self.set_authenticate_continue(bytes);
        self.progress_response(enqueued_response).await;
    }

    pub async fn send_authenticate_finish(&mut self, bytes: &[u8]) {
        let enqueued_response = self.set_authenticate_finish(bytes);
        self.progress_response(enqueued_response).await;
    }

    async fn receive_error(&mut self) -> server::Error {
        let (stream, server) = self.connection_state.greeted();
        let error = stream.next(server).await.unwrap_err();
        match error {
            stream::Error::State(err) => err,
            err => {
                panic!("Server emitted unexpected error: {err:?}");
            }
        }
    }

    pub async fn receive_error_because_expected_crlf_got_lf(&mut self, expected_bytes: &[u8]) {
        let error = self.receive_error().await;
        match error {
            server::Error::ExpectedCrlfGotLf { discarded_bytes } => {
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
            server::Error::MalformedMessage { discarded_bytes } => {
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
            server::Error::LiteralTooLong { discarded_bytes } => {
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
            server::Error::CommandTooLong { discarded_bytes } => {
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
    pub async fn receive_command(&mut self, expected_bytes: &[u8]) {
        let expected_command = self.codecs.decode_command(expected_bytes);
        let (stream, server) = self.connection_state.greeted();
        let event = stream.next(server).await.unwrap();
        match event {
            server::Event::CommandReceived { command } => {
                assert_eq!(expected_command, command);
            }
            event => {
                panic!("Server emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_idle(&mut self, expected_bytes: &[u8]) {
        let expected_command = self.codecs.decode_command(expected_bytes);
        let (stream, server) = self.connection_state.greeted();
        let event = stream.next(server).await.unwrap();
        match event {
            server::Event::IdleCommandReceived { tag } => {
                assert_eq!(expected_command.tag, tag);
            }
            event => {
                panic!("Server emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_idle_done(&mut self) {
        let (stream, server) = self.connection_state.greeted();
        let event = stream.next(server).await.unwrap();
        match event {
            server::Event::IdleDoneReceived => (),
            event => {
                panic!("Server emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_authenticate_command(&mut self, expected_bytes: &[u8]) {
        let expected_command = self.codecs.decode_command(expected_bytes);
        let (stream, server) = self.connection_state.greeted();
        let event = stream.next(server).await.unwrap();
        match event {
            server::Event::CommandAuthenticateReceived {
                command_authenticate,
            } => {
                assert_eq!(expected_command, command_authenticate.into());
            }
            event => {
                panic!("Server emitted unexpected event: {event:?}");
            }
        }
    }

    pub async fn receive_authenticate_data(&mut self, expected_bytes: &[u8]) {
        let expected_authenticate_data = self.codecs.decode_authenticate_data(expected_bytes);
        let (stream, server) = self.connection_state.greeted();
        let event = stream.next(server).await.unwrap();
        match event {
            server::Event::AuthenticateDataReceived { authenticate_data } => {
                assert_eq!(expected_authenticate_data, authenticate_data);
            }
            event => {
                panic!("Server emitted unexpected event: {event:?}");
            }
        }
    }
}

/// Connection state between server and client.
#[allow(clippy::large_enum_variant)]
enum ConnectionState {
    // Connection to client established.
    Connected { stream: Stream },
    // Server greeted client.
    Greeted { stream: Stream, server: Server },
    // Connection dropped.
    Disconnected,
}

impl ConnectionState {
    fn greeted(&mut self) -> (&mut Stream, &mut Server) {
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

/// Enqueued response that can be used for assertions.
pub struct EnqueuedResponse {
    handle: ResponseHandle,
    response: Response<'static>,
}
