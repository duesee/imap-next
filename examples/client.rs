use imap_next::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::Stream,
};
use imap_types::{
    command::{Command, CommandBody},
    core::Tag,
};
use tokio::net::TcpStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::insecure(stream);
    let mut client = ClientFlow::new(ClientFlowOptions::default());

    let greeting = loop {
        match stream.progress(&mut client).await.unwrap() {
            ClientFlowEvent::GreetingReceived { greeting } => break greeting,
            event => println!("unexpected event: {event:?}"),
        }
    };

    println!("received greeting: {greeting:?}");

    let handle = client.enqueue_command(Command {
        tag: Tag::try_from("A1").unwrap(),
        body: CommandBody::login("Al¹cE", "pa²²w0rd").unwrap(),
    });

    loop {
        match stream.progress(&mut client).await.unwrap() {
            ClientFlowEvent::CommandSent {
                handle: got_handle,
                command,
            } => {
                println!("command sent: {got_handle:?}, {command:?}");
                assert_eq!(handle, got_handle);
            }
            ClientFlowEvent::CommandRejected {
                handle: got_handle,
                command,
                status,
            } => {
                println!("command rejected: {got_handle:?}, {command:?}, {status:?}");
                assert_eq!(handle, got_handle);
            }
            ClientFlowEvent::DataReceived { data } => {
                println!("data received: {data:?}");
            }
            ClientFlowEvent::StatusReceived { status } => {
                println!("status received: {status:?}");
            }
            ClientFlowEvent::ContinuationRequestReceived {
                continuation_request,
            } => {
                println!("unexpected continuation request received: {continuation_request:?}");
            }
            event => {
                println!("{event:?}");
            }
        }
    }
}
