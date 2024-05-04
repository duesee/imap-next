use imap_types::{
    auth::AuthMechanism,
    command::{Command, CommandBody},
    core::Tag,
    response::{Greeting, Status},
};
use tokio::net::{TcpListener, TcpStream};

use crate::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
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
            let mut stream = Stream::insecure(stream);
            let mut server = ServerFlow::new(ServerFlowOptions::default(), greeting.clone());

            loop {
                match stream.progress(&mut server).await.unwrap() {
                    ServerFlowEvent::CommandReceived { command } => {
                        let no = Status::no(Some(command.tag), None, "...").unwrap();
                        server.enqueue_status(no);
                    }
                    ServerFlowEvent::CommandAuthenticateReceived {
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
    let mut stream = Stream::insecure(stream);
    let mut client = ClientFlow::new(ClientFlowOptions::default());

    client.enqueue_command(Command::new(Tag::unvalidated("A1"), CommandBody::Capability).unwrap());

    loop {
        match stream.progress(&mut client).await.unwrap() {
            ClientFlowEvent::GreetingReceived {
                greeting: received_greeting,
            } => {
                assert_eq!(greeting, received_greeting)
            }
            ClientFlowEvent::StatusReceived { .. } => {
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
            ClientFlowEvent::AuthenticateStatusReceived { .. } => break,
            _ => {}
        }
    }
}
