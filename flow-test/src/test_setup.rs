use std::net::SocketAddr;

use imap_flow::{client::ClientFlowOptions, server::ServerFlowOptions};
use tokio::net::TcpListener;
use tracing::trace;
use tracing_subscriber::EnvFilter;

use crate::{
    client_tester::ClientTester,
    codecs::Codecs,
    mock::Mock,
    runtime::{Runtime, RuntimeOptions},
    server_tester::ServerTester,
};

/// Contains all parameters for creating a test setup for the server or client side
/// of `imap-flow`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TestSetup {
    pub codecs: Codecs,
    pub server_flow_options: ServerFlowOptions,
    pub client_flow_options: ClientFlowOptions,
    pub runtime_options: RuntimeOptions,
    pub init_logging: bool,
}

impl TestSetup {
    /// Create a test setup to test the client side (mocking the server side).
    pub fn setup_client(self) -> (Runtime, Mock, ClientTester) {
        if self.init_logging {
            init_logging();
        }

        let rt = Runtime::new(self.runtime_options);

        let (server_listener, server_address) = rt.run(bind_address());

        let (server, client) = rt.run2(
            Mock::server(server_listener),
            ClientTester::new(self.codecs, self.client_flow_options, server_address),
        );

        (rt, server, client)
    }

    /// Create a test setup to test the server side (mocking the client side).
    pub fn setup_server(self) -> (Runtime, ServerTester, Mock) {
        if self.init_logging {
            init_logging();
        }

        let rt = Runtime::new(self.runtime_options);

        let (server_listener, server_address) = rt.run(bind_address());

        let (server, client) = rt.run2(
            ServerTester::new(self.codecs, self.server_flow_options, server_listener),
            Mock::client(server_address),
        );

        (rt, server, client)
    }

    /// Create a test setup to test the server side and the client side.
    pub fn setup(self) -> (Runtime, ServerTester, ClientTester) {
        if self.init_logging {
            init_logging();
        }

        let rt = Runtime::new(self.runtime_options);

        let (server_listener, server_address) = rt.run(bind_address());

        let (server, client) = rt.run2(
            ServerTester::new(
                self.codecs.clone(),
                self.server_flow_options,
                server_listener,
            ),
            ClientTester::new(self.codecs, self.client_flow_options, server_address),
        );

        (rt, server, client)
    }
}

impl Default for TestSetup {
    fn default() -> Self {
        Self {
            codecs: Codecs::default(),
            server_flow_options: ServerFlowOptions::default(),
            client_flow_options: ClientFlowOptions::default(),
            runtime_options: RuntimeOptions::default(),
            init_logging: true,
        }
    }
}

fn init_logging() {
    let builder = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .without_time();

    // We use `try_init` because multiple tests might try to initialize the logging
    let _result = builder.try_init();
}

async fn bind_address() -> (TcpListener, SocketAddr) {
    // If we use port 0 the OS will assign us a free port. This is useful because
    // we want to run many tests in parallel and two tests must not use the same port.
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = server_listener.local_addr().unwrap();
    trace!(?server_address, "Bound to address");
    (server_listener, server_address)
}
