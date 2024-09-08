use std::collections::VecDeque;

use imap_next::{
    imap_types::response::{Greeting, Status},
    server::{Event, Options, Server},
    stream::Stream,
};
use tokio::net::TcpListener;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = Stream::new(stream);
    let mut server = Server::new(
        Options::default(),
        Greeting::ok(None, "server (example)").unwrap(),
    );

    loop {
        match stream.next(&mut server).await.unwrap() {
            Event::GreetingSent { greeting } => {
                println!("greeting sent: {greeting:?}");
                break;
            }
            event => println!("unexpected event: {event:?}"),
        }
    }

    let mut handles = VecDeque::new();

    loop {
        match stream.next(&mut server).await.unwrap() {
            Event::CommandReceived { command } => {
                println!("command received: {command:?}");
                handles.push_back(
                    server.enqueue_status(Status::no(Some(command.tag), None, "...").unwrap()),
                );
            }
            Event::ResponseSent {
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
