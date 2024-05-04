use imap_flow::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::Stream,
    types::CommandAuthenticate,
};
use imap_types::response::{CommandContinuationRequest, Greeting, Status};
use tokio::net::TcpListener;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = Stream::insecure(stream);
    let mut server = ServerFlow::new(
        ServerFlowOptions::default(),
        Greeting::ok(None, "server_idle (example)").unwrap(),
    );

    loop {
        match stream.progress(&mut server).await.unwrap() {
            ServerFlowEvent::GreetingSent { .. } => break,
            event => println!("unexpected event: {event:?}"),
        }
    }

    let mut current_authenticate_tag = None;

    loop {
        let event = stream.progress(&mut server).await.unwrap();
        println!("{event:?}");

        // We don't implement any real SASL mechanism in this example.
        let pretend_to_need_more_data = rand::random();

        match event {
            ServerFlowEvent::CommandAuthenticateReceived {
                command_authenticate: CommandAuthenticate { tag, .. },
            } => {
                if pretend_to_need_more_data {
                    server
                        .authenticate_continue(
                            CommandContinuationRequest::basic(None, "I need more data...").unwrap(),
                        )
                        .unwrap();

                    current_authenticate_tag = Some(tag);
                } else {
                    server
                        .authenticate_finish(
                            Status::ok(Some(tag), None, "Thanks, that's already enough!").unwrap(),
                        )
                        .unwrap();
                }
            }
            ServerFlowEvent::AuthenticateDataReceived { .. } => {
                if pretend_to_need_more_data {
                    server
                        .authenticate_continue(
                            CommandContinuationRequest::basic(None, "...more...").unwrap(),
                        )
                        .unwrap();
                } else {
                    let tag = current_authenticate_tag.take().unwrap();

                    server
                        .authenticate_finish(Status::ok(Some(tag), None, "Thanks!").unwrap())
                        .unwrap();
                }
            }
            ServerFlowEvent::CommandReceived { command } => {
                server.enqueue_status(
                    Status::no(Some(command.tag), None, "Please use AUTHENTICATE").unwrap(),
                );
            }
            _ => {}
        }
    }
}
