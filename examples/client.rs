use imap_next::{
    client::{Client, Event, Options},
    imap_types::{
        command::{Command, CommandBody},
        core::Tag,
    },
    stream::Stream,
};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncReadCompatExt;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::new(stream.compat());
    let mut client = Client::new(Options::default());

    let greeting = loop {
        match stream.next(&mut client).await.unwrap() {
            Event::GreetingReceived { greeting } => break greeting,
            event => println!("unexpected event: {event:?}"),
        }
    };

    println!("received greeting: {greeting:?}");

    let handle = client.enqueue_command(Command {
        tag: Tag::try_from("A1").unwrap(),
        body: CommandBody::login("Al¹cE", "pa²²w0rd").unwrap(),
    });

    loop {
        match stream.next(&mut client).await.unwrap() {
            Event::CommandSent {
                handle: got_handle,
                command,
            } => {
                println!("command sent: {got_handle:?}, {command:?}");
                assert_eq!(handle, got_handle);
            }
            Event::CommandRejected {
                handle: got_handle,
                command,
                status,
            } => {
                println!("command rejected: {got_handle:?}, {command:?}, {status:?}");
                assert_eq!(handle, got_handle);
            }
            Event::DataReceived { data } => {
                println!("data received: {data:?}");
            }
            Event::StatusReceived { status } => {
                println!("status received: {status:?}");
            }
            Event::ContinuationRequestReceived {
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
