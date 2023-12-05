//! AUTHENTICATE Example
//!
//! # OK (w/o initial data)
//!
//! ```imap
//! C: A1 AUTHENTICATE PLAIN
//! S: + ...                  // Server needs more data.
//! C: sadjsalkdj==           // Client sends more data.
//! S: A1 OK
//! C: A2 NOOP
//! ```
//!
//! ```imap
//! C: A1 AUTHENTICATE LOGIN
//! S: + ...                  // Server needs more data.
//! C: sadjsalkdj==           // Client sends more data.
//! S: + ...                  // Server needs more data (again).
//! C: sadjsalkdj==           // Client sends more data (again).
//! S: A1 OK
//! C: A2 NOOP
//! ```
//!
//! # OK (w/ initial data, i.e., "SASL-IR")
//!
//! ```imap
//! C: A1 AUTHENTICATE PLAIN sadklsadj== // Client sends more data (in advance).
//! S: A1 OK
//! ```
//!
//! ```imap
//! C: A1 AUTHENTICATE LOGIN sadklsadj== // Client sends more data (in advance).
//! S: + ...                             // Server needs more data (again).
//! C: sadjsalkdj==                      // Client sends more data (again).
//! S: A1 OK
//! ```
//!
//! # NO
//!
//! ```imap
//! C: A1 AUTHENTICATE FOOOOOOO
//! S: A1 NO
//! ```
//!
//! ```imap
//! C: A1 AUTHENTICATE FOOOOOOO
//! S: + <some_insane_challenge>  
//! C: *                          // Client aborts own AUTHENTICATE.
//! ```
//!
//! # IDLE (OK)
//!
//! C: A1 IDLE
//! S: + ...
//! S: * 1 exists
//! S: * 2 exists
//! ...
//! C: DONE
//! S: A1 OK
//! ```
//! 
//! # IDLE (NO)
//!
//! C: A1 IDLE
//! S: A1 NO
//! ```

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
            ServerFlowEvent::AuthenticationProgress { tag, auth_data } => match auth_data {
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
