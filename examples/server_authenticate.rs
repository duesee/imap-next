use std::error::Error;

use imap_codec::imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    response::{CommandContinuationRequest, Greeting, Status},
};
use imap_flow::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::AnyStream,
    types::CommandAuthenticate,
};
use tokio::net::TcpListener;

struct Sasl;

impl Sasl {
    fn step(&mut self, _: AuthenticateData) -> Result<State, ()> {
        // Mock
        Ok(State::Finished)
    }
}

enum State {
    #[allow(unused)]
    Incomplete,
    Finished,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let stream = {
        let listener = TcpListener::bind("127.0.0.1:12345").await?;

        let (stream, _) = listener.accept().await?;

        stream
    };

    let (mut server, _) = ServerFlow::send_greeting(
        AnyStream::new(stream),
        ServerFlowOptions::default(),
        Greeting::ok(None, "Hello, World!")?,
    )
    .await?;

    let mut dance = Sasl;

    let mut current_tag = None;

    loop {
        let event = server.progress().await?;
        println!("{event:?}");

        match event {
            ServerFlowEvent::CommandAuthenticateReceived {
                command_authenticate:
                    CommandAuthenticate {
                        tag,
                        mechanism,
                        initial_response,
                    },
            } => match mechanism {
                AuthMechanism::Plain => {
                    if let Some(_initial_response) = initial_response {
                        server
                            .authenticate_finish(Status::ok(Some(tag), None, "...")?)
                            .map_err(|_| "bad state")?;
                    } else {
                        current_tag = Some(tag);
                        server
                            .authenticate_continue(CommandContinuationRequest::basic(None, "...")?)
                            .map_err(|_| "bad state")?;
                    }
                }
                _ => {
                    server
                        .authenticate_finish(Status::no(Some(tag), None, "...")?)
                        .map_err(|_| "bad state")?;
                }
            },
            ServerFlowEvent::AuthenticateDataReceived { authenticate_data } => {
                match current_tag.clone() {
                    Some(tag) => match authenticate_data {
                        AuthenticateData::Continue(..) => match dance.step(authenticate_data) {
                            Ok(state) => match state {
                                State::Incomplete => {
                                    server
                                        .authenticate_continue(CommandContinuationRequest::basic(
                                            None, "...",
                                        )?)
                                        .map_err(|_| "bad state")?;
                                }
                                State::Finished => {
                                    server
                                        .authenticate_finish(Status::ok(Some(tag), None, "...")?)
                                        .map_err(|_| "bad state")?;
                                }
                            },
                            Err(_) => {
                                server
                                    .authenticate_finish(Status::no(Some(tag), None, "...")?)
                                    .map_err(|_| "bad state")?;
                            }
                        },
                        AuthenticateData::Cancel => {
                            server
                                .authenticate_finish(Status::no(Some(tag), None, "...")?)
                                .map_err(|_| "bad state")?;
                        }
                    },
                    None => {
                        println!("Error");
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}
