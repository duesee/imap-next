use std::collections::VecDeque;

use imap_next::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::Stream,
};
use imap_types::response::{Greeting, Status};
use tokio::net::TcpListener;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = Stream::insecure(stream);
    let mut server = ServerFlow::new(
        ServerFlowOptions::default(),
        Greeting::ok(None, "server (example)").unwrap(),
    );

    loop {
        match stream.progress(&mut server).await.unwrap() {
            ServerFlowEvent::GreetingSent { greeting } => {
                println!("greeting sent: {greeting:?}");
                break;
            }
            event => println!("unexpected event: {event:?}"),
        }
    }

    let mut handles = VecDeque::new();

    loop {
        match stream.progress(&mut server).await.unwrap() {
            ServerFlowEvent::CommandReceived { command } => {
                println!("command received: {command:?}");
                handles.push_back(
                    server.enqueue_status(Status::no(Some(command.tag), None, "...").unwrap()),
                );
            }
            ServerFlowEvent::ResponseSent {
                handle: got_handle,
                response,
            } => {
                println!("response sent: {response:?}");
                assert_eq!(handles.pop_front(), Some(got_handle));
            }
            event => {
                println!("{event:?}");
            }
        }
    }
}
