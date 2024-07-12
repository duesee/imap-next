use std::io::BufRead;

use imap_next::{
    client::{Client, Event, Options},
    imap_types::{
        command::{Command, CommandBody},
        core::Tag,
        response::{Status, Tagged},
    },
    stream::Stream,
};
use tokio::{net::TcpStream, sync::mpsc::Receiver};

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

    println!("Press ENTER to stop IDLE");
    let mut lines = Lines::new();

    let tag = Tag::unvalidated("A1");
    let _handle = client.enqueue_command(Command {
        tag: tag.clone(),
        body: CommandBody::Idle,
    });

    loop {
        tokio::select! {
            event = stream.next(&mut client) => {
                match event.unwrap() {
                    Event::IdleCommandSent { .. } => {
                        println!("IDLE command sent")
                    },
                    Event::IdleAccepted { continuation_request, .. } => {
                        println!("IDLE accepted: {continuation_request:?}");
                    },
                    Event::IdleRejected { status, .. } => {
                        println!("IDLE rejected: {status:?}");
                        break;
                    },
                    Event::IdleDoneSent { .. } => {
                        println!("IDLE DONE sent");
                        break;
                    },
                    Event::DataReceived { data } => {
                        println!("Data received: {data:?}")
                    },
                    Event::StatusReceived { status } => {
                        println!("Status received: {status:?}")
                    },
                    event => {
                        println!("Unknown event received: {event:?}");
                    }
                }
            }
            _ = lines.next() => {
                if client.set_idle_done().is_some() {
                    println!("Triggered IDLE DONE");
                } else {
                    println!("Can't trigger IDLE DONE now");
                }
            }
        }
    }

    loop {
        match stream.next(&mut client).await.unwrap() {
            ref event @ Event::StatusReceived {
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

    pub async fn next(&mut self) -> String {
        self.receiver.recv().await.unwrap()
    }
}
