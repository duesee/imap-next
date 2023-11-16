mod common;

use common::Lines;
use imap_codec::imap_types::{
    command::{Command, CommandBody},
    core::Tag,
    response::{Status, Tagged},
};
use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
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

    println!("Press ENTER to STOP IDLING");
    let mut lines = Lines::new();

    let tag = Tag::unvalidated("A1");
    let _handle = client.enqueue_command(Command::new(tag.clone(), CommandBody::Idle).unwrap());

    loop {
        tokio::select! {
            event = client.progress() => {
                match event.unwrap() {
                    ClientFlowEvent::CommandSent { .. } => {
                        // Note: This event only occurs AFTER the client sent "DONE".
                        // This means, `client.stop_idling()` must be called before.
                        println!("IDLE sent");
                    }
                    ClientFlowEvent::CommandRejected { .. } => {
                        println!("IDLE rejected");
                        break;
                    }
                    ClientFlowEvent::StatusReceived { status: Status::Tagged(Tagged { tag: got_tag, .. }) } => {
                        if got_tag == tag {
                            println!("IDLE finished");
                            break;
                        }
                    }
                    event => {
                        println!("{event:?}");
                    }
                }
            }
            _ = lines.progress() => {
                println!("DONE (enter)");
                if client.is_idling() {
                    client.stop_idling().unwrap();
                }
            }
        }
    }
}
