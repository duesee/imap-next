use std::io::BufRead;

use imap_next::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::Stream,
};
use imap_types::{
    command::{Command, CommandBody},
    core::Tag,
    response::{Status, Tagged},
};
use tokio::{net::TcpStream, sync::mpsc::Receiver};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::insecure(stream);
    let mut client = ClientFlow::new(ClientFlowOptions::default());

    loop {
        match stream.progress(&mut client).await.unwrap() {
            ClientFlowEvent::GreetingReceived { .. } => break,
            event => println!("unexpected event: {event:?}"),
        }
    }

    println!("Press ENTER to stop IDLE");
    let mut lines = Lines::new();

    let tag = Tag::unvalidated("A1");
    let _handle = client.enqueue_command(Command {
        tag: tag.clone(),
        body: CommandBody::Idle,
    });

    loop {
        tokio::select! {
            event = stream.progress(&mut client) => {
                match event.unwrap() {
                    ClientFlowEvent::IdleCommandSent { .. } => {
                        println!("IDLE command sent")
                    },
                    ClientFlowEvent::IdleAccepted { continuation_request, .. } => {
                        println!("IDLE accepted: {continuation_request:?}");
                    },
                    ClientFlowEvent::IdleRejected { status, .. } => {
                        println!("IDLE rejected: {status:?}");
                        break;
                    },
                    ClientFlowEvent::IdleDoneSent { .. } => {
                        println!("IDLE DONE sent");
                        break;
                    },
                    ClientFlowEvent::DataReceived { data } => {
                        println!("Data received: {data:?}")
                    },
                    ClientFlowEvent::StatusReceived { status } => {
                        println!("Status received: {status:?}")
                    },
                    event => {
                        println!("Unknown event received: {event:?}");
                    }
                }
            }
            _ = lines.progress() => {
                if client.set_idle_done().is_some() {
                    println!("Triggered IDLE DONE");
                } else {
                    println!("Can't trigger IDLE DONE now");
                }
            }
        }
    }

    loop {
        match stream.progress(&mut client).await.unwrap() {
            ref event @ ClientFlowEvent::StatusReceived {
                status:
                    Status::Tagged(Tagged {
                        tag: ref got_tag, ..
                    }),
            } if *got_tag == tag => {
                println!("Status for IDLE received: {event:?}");
                break;
            }
            event => {
                println!("Unknown event received: {event:?}");
            }
        }
    }
}

struct Lines {
    receiver: Receiver<String>,
}

impl Lines {
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        tokio::task::spawn_blocking(move || loop {
            for line in std::io::stdin().lock().lines() {
                sender.blocking_send(line.unwrap()).unwrap();
            }
        });

        Self { receiver }
    }

    pub async fn progress(&mut self) -> String {
        self.receiver.recv().await.unwrap()
    }
}
