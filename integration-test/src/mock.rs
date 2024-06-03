use std::net::SocketAddr;

use bstr::{BStr, ByteSlice};
use bytes::{Buf, BytesMut};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tracing::trace;

/// Mocks either the server or client.
///
/// This mock doesn't know any IMAP semantics. Instead it provides direct access to the
/// TCP connection. Therefore the correctness of the test depends on the correctness
/// of the test data.
pub struct Mock {
    role: Role,
    stream: TcpStream,
    read_buffer: BytesMut,
}

impl Mock {
    pub async fn server(server_listener: TcpListener) -> Self {
        let role = Role::Server;
        let (stream, client_address) = server_listener.accept().await.unwrap();
        trace!(?role, ?client_address, "Mock accepts connection");
        Self {
            role,
            stream,
            read_buffer: BytesMut::default(),
        }
    }

    pub async fn client(server_address: SocketAddr) -> Self {
        let role = Role::Client;
        let stream = TcpStream::connect(server_address).await.unwrap();
        trace!(?role, ?server_address, "Mock is connected");
        Self {
            role,
            stream,
            read_buffer: BytesMut::default(),
        }
    }

    pub async fn send(&mut self, bytes: &[u8]) {
        trace!(
            role = ?self.role,
            bytes = ?BStr::new(bytes),
            "Mock writes bytes"
        );
        self.stream.write_all(bytes).await.unwrap();
    }

    pub async fn receive(&mut self, expected_bytes: &[u8]) {
        loop {
            let bytes = &self.read_buffer[..];
            trace!(
                role = ?self.role,
                read_bytes = ?BStr::new(bytes),
                "Mock reads bytes"
            );

            if bytes.len() < expected_bytes.len() {
                assert_eq!(expected_bytes[..bytes.len()].as_bstr(), bytes.as_bstr());

                self.stream.read_buf(&mut self.read_buffer).await.unwrap();
            } else {
                assert_eq!(
                    expected_bytes.as_bstr(),
                    bytes[..expected_bytes.len()].as_bstr()
                );

                self.read_buffer.advance(expected_bytes.len());
                break;
            }
        }
    }
}

#[derive(Debug)]
enum Role {
    Server,
    Client,
}
