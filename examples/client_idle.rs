mod common;

use common::Lines;
use imap_codec::imap_types::{
    command::{Command, CommandBody},
    core::Tag,
    response::{Status, Tagged},
};
use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowEventIdle, ClientFlowOptions},
    stream::AnyStream,
};
use tokio::net::TcpStream;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();

    let (mut client, _) =
        ClientFlow::receive_greeting(AnyStream::new(stream), ClientFlowOptions::default())
            .await
            .unwrap();

    println!("Press ENTER to stop IDLE");
    let mut lines = Lines::new();

    // TODO: CommandBody::Idle must not be enqueued. This must be fixed in `imap-types`.
    let _handle = client.enqueue_command(
        Command::new("A1", CommandBody::login("alice", "pa²²w0rd").unwrap()).unwrap(),
    );

    let tag = Tag::unvalidated("A2");
    let mut client = client.idle(tag.clone());

    let mut client = loop {
        tokio::select! {
            event = client.progress() => {
                match event.unwrap() {
                    ClientFlowEventIdle::Event(event) => match event {
                        ClientFlowEvent::CommandSent { command, .. } => {
                            println!("sent: {command:?}");
                        },
                        ClientFlowEvent::CommandRejected { command, .. } => {
                            println!("rejected: {command:?}");
                        }
                        ClientFlowEvent::DataReceived { data } => {
                            println!("{data:?}");
                        }
                        ClientFlowEvent::StatusReceived { status } => {
                            println!("{status:?}");
                        }
                        ClientFlowEvent::ContinuationReceived { continuation } => {
                            println!("{continuation:?}");
                        }
                    }
                    ClientFlowEventIdle::Finished(client) => {
                        // Note: Calling `ClientFlowIdle::progress()` again will fail.
                        break client;
                    }
                }
            }
            _ = lines.progress(), if !client.is_done() => {
                println!("break due to enter");
                client.done();
            }
        }
    };

    loop {
        match client.progress().await.unwrap() {
            ref event @ ClientFlowEvent::StatusReceived {
                status:
                    Status::Tagged(Tagged {
                        tag: ref got_tag, ..
                    }),
            } if *got_tag == tag => {
                println!("{event:?}");
                break;
            }
            event => {
                println!("{event:?}");
            }
        }
    }
}
