use imap_codec::imap_types::{
    command::{Command, CommandBody},
    core::Tag,
    response::Status,
    secret::Secret,
};
use imap_flow::{
    auth::Authenticate,
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::AnyStream,
};
use tokio::net::TcpStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();

    let (mut client, greeting) =
        ClientFlow::receive_greeting(AnyStream::new(stream), ClientFlowOptions::default())
            .await
            .unwrap();
    println!("received greeting: {greeting:?}");

    // TODO: Forbid Command::Authenticate.
    let handle1 = client.enqueue_command(Command {
        tag: Tag::try_from("A1").unwrap(),
        body: CommandBody::Capability,
    });

    loop {
        match client.progress().await.unwrap() {
            ClientFlowEvent::CommandSent {
                handle: got_handle,
                tag,
            } => {
                println!("command sent: {got_handle:?}, {tag:?}");
                assert_eq!(handle1, got_handle);
            }
            ClientFlowEvent::CommandRejected {
                handle: got_handle,
                tag,
                status,
            } => {
                println!("command rejected: {got_handle:?}, {tag:?}, {status:?}");
                assert_eq!(handle1, got_handle);
            }
            ClientFlowEvent::DataReceived { data } => {
                println!("data received: {data:?}");
            }
            ClientFlowEvent::StatusReceived { status } => match status {
                Status::Tagged(_) => {
                    println!("command completion result response received: {status:?}");
                    break;
                }
                _ => {
                    println!("status received: {status:?}");
                }
            },
        }
    }

    let handle2 = client.enqueue_command_authenticate(
        Tag::try_from("A2").unwrap(),
        Authenticate::Plain {
            username: b"alice".to_vec(),
            password: Secret::new(b"password".to_vec()),
        },
    );

    loop {
        match client.progress().await.unwrap() {
            ClientFlowEvent::CommandSent {
                handle: got_handle,
                tag,
            } => {
                println!("command sent: {got_handle:?}, {tag:?}");
                assert_eq!(handle2, got_handle);
            }
            ClientFlowEvent::CommandRejected {
                handle: got_handle,
                tag,
                status,
            } => {
                println!("command rejected: {got_handle:?}, {tag:?}, {status:?}");
                assert_eq!(handle2, got_handle);
            }
            ClientFlowEvent::DataReceived { data } => {
                println!("data received: {data:?}");
            }
            ClientFlowEvent::StatusReceived { status } => match status {
                Status::Tagged(_) => {
                    println!("command completion result response received: {status:?}");
                    break;
                }
                _ => {
                    println!("status received: {status:?}");
                }
            },
        }
    }
}
