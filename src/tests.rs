use imap_codec::imap_types::{
    auth::AuthMechanism,
    command::{Command, CommandBody},
    core::Tag,
    response::{Greeting, Status},
};
use tokio::net::{TcpListener, TcpStream};

use crate::{
    client::{self, Client},
    server::{self, Server},
    stream::Stream,
};

#[tokio::test]
async fn self_test() {
    let greeting = Greeting::ok(None, "Hello, World!").unwrap();

    // Port 0 means "pick any available port"
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = {
        let greeting = greeting.clone();

        async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = Stream::new(stream);
            let mut server = Server::new(server::Options::default(), greeting.clone());

            loop {
                match stream.next(&mut server).await.unwrap() {
                    server::Event::CommandReceived { command } => {
                        let no = Status::no(Some(command.tag), None, "...").unwrap();
                        server.enqueue_status(no);
                    }
                    server::Event::CommandAuthenticateReceived {
                        command_authenticate,
                    } => {
                        let no = Status::no(Some(command_authenticate.tag), None, "...").unwrap();
                        server.enqueue_status(no);
                    }
                    _ => {}
                }
            }
        }
    };

    #[allow(clippy::let_underscore_future)]
    let _ = tokio::task::spawn(server);

    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut stream = Stream::new(stream);
    let mut client = Client::new(client::Options::default());

    client.enqueue_command(Command::new(Tag::unvalidated("A1"), CommandBody::Capability).unwrap());

    loop {
        match stream.next(&mut client).await.unwrap() {
            client::Event::GreetingReceived {
                greeting: received_greeting,
            } => {
                assert_eq!(greeting, received_greeting)
            }
            client::Event::StatusReceived { .. } => {
                client.enqueue_command(
                    Command::new(
                        Tag::unvalidated("A2"),
                        CommandBody::Authenticate {
                            mechanism: AuthMechanism::Plain,
                            initial_response: None,
                        },
                    )
                    .unwrap(),
                );
            }
            client::Event::AuthenticateStatusReceived { .. } => break,
            _ => {}
        }
    }
}
