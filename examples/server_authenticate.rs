use imap_codec::imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    response::{CommandContinuationRequest, Greeting, Status},
};
use imap_flow::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::AnyStream,
    types::AuthenticateCommandData,
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
async fn main() {
    let stream = {
        let listener = TcpListener::bind("127.0.0.1:12345").await.unwrap();

        let (stream, _) = listener.accept().await.unwrap();

        stream
    };

    let (mut server, _) = ServerFlow::send_greeting(
        AnyStream::new(stream),
        ServerFlowOptions::default(),
        Greeting::ok(None, "Hello, World!").unwrap(),
    )
    .await
    .unwrap();

    let mut dance = Sasl;

    let mut tag = None;

    loop {
        match server.progress().await.unwrap() {
            ServerFlowEvent::AuthenticationStart {
                authenticate_command_data:
                    AuthenticateCommandData {
                        tag: got_tag,
                        mechanism,
                        initial_response,
                    },
            } => match mechanism {
                AuthMechanism::Plain => {
                    if let Some(_initial_response) = initial_response {
                        server
                            .authenticate_finish(Status::ok(Some(got_tag), None, "...").unwrap())
                            .unwrap();
                    } else {
                        tag = Some(got_tag);
                        server
                            .authenticate_continue(
                                CommandContinuationRequest::basic(None, "...").unwrap(),
                            )
                            .unwrap();
                    }
                }
                _ => {
                    server
                        .authenticate_finish(Status::no(Some(got_tag), None, "...").unwrap())
                        .unwrap();
                }
            },
            ServerFlowEvent::AuthenticationProgress { authenticate_data } => match tag.clone() {
                Some(tag) => match authenticate_data {
                    AuthenticateData::Continue(..) => match dance.step(authenticate_data) {
                        Ok(state) => match state {
                            State::Incomplete => {
                                server
                                    .authenticate_continue(
                                        CommandContinuationRequest::basic(None, "...").unwrap(),
                                    )
                                    .unwrap();
                            }
                            State::Finished => {
                                server
                                    .authenticate_finish(
                                        Status::ok(Some(tag), None, "...").unwrap(),
                                    )
                                    .unwrap();
                            }
                        },
                        Err(_) => {
                            server
                                .authenticate_finish(Status::no(Some(tag), None, "...").unwrap())
                                .unwrap();
                        }
                    },
                    AuthenticateData::Cancel => {
                        server
                            .authenticate_finish(Status::no(Some(tag), None, "...").unwrap())
                            .unwrap();
                    }
                },
                None => {
                    println!("Error");
                    break;
                }
            },
            event => {
                println!("{event:?}");
            }
        }
    }
}
