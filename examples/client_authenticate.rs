use std::collections::VecDeque;

use imap_codec::imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::{Command, CommandBody},
    secret::Secret,
};
use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::AnyStream,
};
use tag_generator::TagGenerator;
use tokio::net::TcpStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut tag_generator = TagGenerator::new();

    let stream = AnyStream::new(TcpStream::connect("127.0.0.1:12345").await.unwrap());

    let (mut client, greeting) = ClientFlow::receive_greeting(stream, ClientFlowOptions::default())
        .await
        .unwrap();

    println!("{greeting:?}");

    let tag = tag_generator.generate();
    client.enqueue_command(Command {
        tag: tag.clone(),
        body: CommandBody::authenticate(AuthMechanism::Login),
    });

    let mut authenticate_data = VecDeque::from([
        AuthenticateData::Continue(Secret::new(b"alice".to_vec())),
        AuthenticateData::Continue(Secret::new(b"password".to_vec())),
    ]);

    loop {
        match client.progress().await.unwrap() {
            ClientFlowEvent::AuthenticationContinue {
                handle,
                continuation,
            } => {
                println!("AuthenticationContinue: {continuation:?}");

                client.enqueue_command(Command {
                    tag: tag_generator.generate(),
                    body: CommandBody::Noop,
                });
                if let Some(authenticate_data) = authenticate_data.pop_front() {
                    client.authenticate_continue(authenticate_data);
                } else {
                    client.authenticate_continue(AuthenticateData::Cancel);
                }
            }
            ClientFlowEvent::AuthenticationAccepted { .. } => {
                if authenticate_data.is_empty() {
                    println!("Success");
                    //break;
                } else {
                    println!("Unexpected success!!!!1^111");
                    //break;
                }
            }
            ClientFlowEvent::AuthenticationRejected {
                handle,
                authenticate_command_data,
                status,
            } => {
                println!("Failed {status:?}");
                //break;
            }
            event => {
                println!("unexpected event: {event:?}");
            }
        }
    }
}
