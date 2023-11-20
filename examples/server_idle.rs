mod common;

use std::io::BufRead;

use common::Lines;
use imap_codec::{
    imap_types::{
        command::{Command, CommandBody},
        core::Tag,
        response::{CommandContinuationRequest, Data, Greeting, Status},
    },
    CommandCodec, IdleDoneCodec,
};
use imap_flow::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowEventIdle, ServerFlowOptions},
    stream::AnyStream,
};
use tokio::net::TcpListener;

enum ServerWrapper {
    Normal(ServerFlow<CommandCodec>),
    Idle((Tag<'static>, ServerFlow<IdleDoneCodec>)),
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stream = {
        let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();

        let (stream, _) = listener.accept().await.unwrap();

        stream
    };

    let (server, _) = ServerFlow::send_greeting(
        AnyStream::new(stream),
        ServerFlowOptions::default(),
        Greeting::ok(None, "Hello, World!").unwrap(),
    )
    .await
    .unwrap();

    println!("Press ENTER to send server data");
    let mut lines = Lines::new();

    // We use an enum to switch between [`Server::Normal`] and [`Server::Idle`] state.
    let mut server_wrapper = ServerWrapper::Normal(server);

    loop {
        server_wrapper = match server_wrapper {
            ServerWrapper::Normal(mut server) => loop {
                tokio::select! {
                    event = server.progress() => {
                        match event.unwrap() {
                            ServerFlowEvent::CommandReceived { command: Command { tag, body } } => match body {
                                CommandBody::Idle => {
                                    let continuation = CommandContinuationRequest::basic(None, "Let's IDLE (press ENTER in server)").unwrap();
                                    // TODO: Remember tag in server?
                                    break ServerWrapper::Idle((tag, server.accept_idle(continuation)));
                                }
                                _ => {
                                    let status = Status::no(Some(tag), None, "I'll only accept IDLE").unwrap();
                                    server.enqueue_status(status);
                                }
                            }
                            event => {
                                println!("{event:?}");
                            }
                        }
                    }
                    _ = lines.progress() => {
                        println!("I'll only send updates during IDLE");
                    }
                }
            },
            ServerWrapper::Idle((tag, mut server)) => loop {
                tokio::select! {
                    event = server.progress() => {
                        match event.unwrap() {
                            ServerFlowEventIdle::ResponseSent { response, .. } => {
                                println!("{response:?}");
                            }
                            ServerFlowEventIdle::Finished(token) => {
                                server.enqueue_status(Status::ok(Some(tag), None, "Thanks!").unwrap());
                                break ServerWrapper::Normal(server.finish_idle(token));
                            }
                        }
                    }
                    _ = lines.progress() => {
                        server.enqueue_data(Data::Exists(1337));
                    }
                }
            },
        }
    }
}
