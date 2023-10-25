use imap_codec::imap_types::{
    command::{Command, CommandBody},
    core::Tag,
};
use imap_flow::{
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

    let handle = client.enqueue_command(Command {
        tag: Tag::try_from("A1").unwrap(),
        body: CommandBody::Noop,
    });

    loop {
        match client.progress().await.unwrap() {
            ClientFlowEvent::CommandSent {
                handle: got_handle,
                tag,
            } => {
                println!("command sent: {got_handle:?}, {tag:?}");
                assert_eq!(handle, got_handle);
            }
            ClientFlowEvent::CommandRejected {
                handle: got_handle,
                tag,
                status,
            } => {
                println!("command rejected: {got_handle:?}, {tag:?}, {status:?}");
                assert_eq!(handle, got_handle);
            }
            ClientFlowEvent::DataReceived { data } => {
                println!("data received: {data:?}");
            }
            ClientFlowEvent::StatusReceived { status } => {
                println!("status received: {status:?}");
            }
        }
    }
}
