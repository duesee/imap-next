use imap_codec::imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::{Command, CommandBody},
    core::Tag,
    secret::Secret,
};
use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::AnyStream,
};
use tokio::net::TcpStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = AnyStream::new(TcpStream::connect("127.0.0.1:12345").await.unwrap());

    let (mut client, greeting) = ClientFlow::receive_greeting(stream, ClientFlowOptions::default())
        .await
        .unwrap();

    println!("{greeting:?}");

    let tag = Tag::unvalidated("A1");
    client.enqueue_command(Command {
        tag: tag.clone(),
        body: CommandBody::authenticate(AuthMechanism::Plain),
    });

    loop {
        match client.progress().await.unwrap() {
            ClientFlowEvent::AuthenticationContinue {
                handle,
                continuation,
            } => {
                client.authenticate_continue(AuthenticateData::Continue(Secret::new(
                    b"Foo".to_vec(),
                )));
            }
            ClientFlowEvent::AuthenticationAccepted => {}
            ClientFlowEvent::AuthenticationRejected => {}
            ClientFlowEvent::CommandSent { .. } => {}
            event => {
                println!("unexpected event: {event:?}");
            }
        }
    }
}
