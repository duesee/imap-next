use imap_codec::imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    response::{CommandContinuationRequest, Greeting, Status},
};
use imap_flow::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::AnyStream,
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

    loop {
        match server.progress().await.unwrap() {
            ServerFlowEvent::AuthenticationStart {
                tag,
                mechanism,
                initial_response,
            } => match mechanism {
                AuthMechanism::Plain => {
                    if let Some(_initial_response) = initial_response {
                        server.authenticate_finish(Status::ok(Some(tag), None, "...").unwrap());
                    } else {
                        server.authenticate_continue(
                            CommandContinuationRequest::basic(None, "...").unwrap(),
                        );
                    }
                }
                _ => {
                    server.authenticate_reject(Status::no(Some(tag), None, "...").unwrap());
                }
            },
            ServerFlowEvent::AuthenticationProgress { tag, authenticate_data: auth_data } => match auth_data {
                AuthenticateData::Continue(..) => match dance.step(auth_data) {
                    Ok(state) => match state {
                        State::Incomplete => server.authenticate_continue(
                            CommandContinuationRequest::basic(None, "...").unwrap(),
                        ),
                        State::Finished => {
                            server.authenticate_finish(Status::ok(Some(tag), None, "...").unwrap())
                        }
                    },
                    Err(_) => {
                        server.authenticate_reject(Status::no(Some(tag), None, "...").unwrap());
                    }
                },
                AuthenticateData::Cancel => {
                    server.authenticate_finish(Status::no(Some(tag), None, "...").unwrap())
                }
            },
            event => {
                println!("{event:?}");
            }
        }
    }
}
