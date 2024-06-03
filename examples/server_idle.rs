use std::{io::BufRead, num::NonZeroU32};

use imap_next::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::Stream,
};
use imap_types::{
    core::Text,
    response::{
        CommandContinuationRequest, Data, Greeting, Status, StatusBody, StatusKind, Tagged,
    },
};
use tokio::{net::TcpListener, sync::mpsc::Receiver};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = Stream::insecure(stream);
    let mut server = ServerFlow::new(
        ServerFlowOptions::default(),
        Greeting::ok(None, "server_idle (example)").unwrap(),
    );

    loop {
        match stream.progress(&mut server).await.unwrap() {
            ServerFlowEvent::GreetingSent { .. } => break,
            event => println!("unexpected event: {event:?}"),
        }
    }

    println!("Please enter 'ok', 'no', or 'expunge'");
    let mut lines = Lines::new();

    let mut current_idle_tag = None;

    loop {
        tokio::select! {
            event = stream.progress(&mut server) => {
                match event.unwrap() {
                    ServerFlowEvent::IdleCommandReceived { tag } => {
                        println!("IDLE received");

                        current_idle_tag = Some(tag);
                    },
                    ServerFlowEvent::IdleDoneReceived => {
                        println!("IDLE DONE received");

                        if let Some(tag) = current_idle_tag.take() {
                            let status = Status::Tagged(Tagged {
                                tag,
                                body: StatusBody {
                                    kind: StatusKind::Ok,
                                    code: None,
                                    text: Text::try_from("...").unwrap()
                                },
                            });
                            server.enqueue_status(status);
                        }
                    },
                    event => {
                        println!("Event received: {event:?}");
                    }
                }
            }
            line = lines.progress() => {
                match line.as_ref() {
                    "ok" => {
                        let cont = CommandContinuationRequest::basic(None, "...").unwrap();
                        if server.idle_accept(cont).is_ok() {
                            println!("IDLE accepted");
                        } else {
                            println!("IDLE can't be accepted now");
                        }
                    }
                    "no" => {
                        let Some(tag) = current_idle_tag.clone() else {
                            println!("IDLE can't be rejected now");
                            continue;
                        };
                        let status = Status::Tagged(Tagged {
                            tag,
                            body: StatusBody {
                                kind: StatusKind::No,
                                code: None,
                                text: Text::try_from("...").unwrap()
                            },
                        });
                        if server.idle_reject(status).is_ok() {
                            println!("IDLE rejected");
                        } else {
                            println!("IDLE can't be rejected now");
                        }
                    }
                    "expunge" => {
                        let data = Data::Expunge(NonZeroU32::new(1).unwrap());
                        server.enqueue_data(data);
                        println!("Send EXPUNGE");
                    }
                    _ => println!("Please enter 'ok', 'no', or 'expunge'"),
                }
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
