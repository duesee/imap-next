use std::collections::VecDeque;

use imap_next::{
    client::{Client, Event, Options},
    imap_types::{
        auth::{AuthMechanism, AuthenticateData},
        command::{Command, CommandBody},
        core::TagGenerator,
    },
    stream::Stream,
};
use tokio::net::TcpStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::insecure(stream);
    let mut client = Client::new(Options::default());

    loop {
        match stream.next(&mut client).await.unwrap() {
            Event::GreetingReceived { .. } => break,
            event => println!("unexpected event: {event:?}"),
        }
    }

    let mut tag_generator = TagGenerator::new();

    let tag = tag_generator.generate();
    client.enqueue_command(Command {
        tag: tag.clone(),
        body: CommandBody::authenticate(AuthMechanism::Login),
    });

    let mut authenticate_data = VecDeque::from([
        AuthenticateData::r#continue(b"alice".to_vec()),
        AuthenticateData::r#continue(b"password".to_vec()),
    ]);

    loop {
        let event = stream.next(&mut client).await.unwrap();
        println!("{event:?}");

        match event {
            Event::AuthenticateContinuationRequestReceived { .. } => {
                if let Some(authenticate_data) = authenticate_data.pop_front() {
                    client.set_authenticate_data(authenticate_data).unwrap();
                } else {
                    client
                        .set_authenticate_data(AuthenticateData::Cancel)
                        .unwrap();
                }
            }
            Event::AuthenticateStatusReceived { .. } => {
                break;
            }
            _ => {}
        }
    }
}
