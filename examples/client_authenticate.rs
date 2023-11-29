#[path = "common/common.rs"]
mod common;

use argh::FromArgs;
use common::TlsStream;
use imap_codec::imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    command::{Command, CommandBody},
    core::Tag,
    response::{CommandContinuationRequest, Status, Tagged},
    secret::Secret,
};
use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::AnyStream,
};
use rsasl::prelude::*;
use tokio::net::TcpStream;
use tracing::{error, info, info_span, warn, Level};

#[derive(Debug, FromArgs)]
/// Client (Authenticate).
struct Arguments {
    /// host
    #[argh(positional)]
    host: String,

    /// port
    #[argh(positional)]
    port: u16,

    /// username
    #[argh(positional)]
    username: String,

    /// password
    #[argh(positional)]
    password: String,

    /// allow authentication mechanism (can be used multiple times)
    #[argh(option)]
    auth: Vec<String>,

    /// allow initial response (SASL-IR)
    #[argh(switch)]
    ir: bool,

    /// don't use TLS (insecure)
    #[argh(switch)]
    insecure: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_target(false)
        .with_line_number(true)
        .without_time()
        .init();

    let args: Arguments = argh::from_env();

    let stream = if args.insecure {
        AnyStream::new(TcpStream::connect((args.host, args.port)).await.unwrap())
    } else {
        AnyStream::new(TlsStream::connect(&args.host, args.port).await)
    };

    // The client will typically agree with the server on an authentication mechanism.
    // However, we want to specify a fixed set of authentication mechanisms here.
    let authentication_mechanisms: Vec<_> = args
        .auth
        .iter()
        .map(|mechanism| mechanism.to_uppercase())
        .collect();

    let authentication_mechanisms: Vec<_> = authentication_mechanisms
        .iter()
        .filter_map(|mechanism| Mechname::parse(mechanism.as_bytes()).ok())
        .collect();

    info!(?authentication_mechanisms);

    let mut sasl_session = {
        // Note: Depending on what we provide here, different authentication mechanisms will be supported.
        let sasl = SASLClient::new(
            SASLConfig::with_credentials(None, args.username.into(), args.password.into()).unwrap(),
        );

        sasl.start_suggested(&authentication_mechanisms).unwrap()
    };

    let chosen_mechanism =
        AuthMechanism::try_from(sasl_session.get_mechname().to_string()).unwrap();

    info!(?chosen_mechanism);

    let (mut client, greeting) = ClientFlow::receive_greeting(stream, ClientFlowOptions::default())
        .await
        .unwrap();

    info!(?greeting);

    let mut state = None;

    let body = if sasl_session.are_we_first() && args.ir {
        let mut out = Vec::new();
        state = Some(sasl_session.step(None, &mut out).unwrap());
        CommandBody::authenticate_with_ir(chosen_mechanism, out)
    } else {
        CommandBody::authenticate(chosen_mechanism)
    };

    let tag = Tag::unvalidated("A1");
    client.enqueue_command(Command {
        tag: tag.clone(),
        body,
    });

    loop {
        let span = info_span!("loop", ?state);
        let _enter = span.enter();

        let event = client.progress().await.unwrap();
        info!(?event);

        match event {
            ClientFlowEvent::CommandSent { .. } => { /* Expected */ }
            ClientFlowEvent::ContinuationReceived { continuation } => {
                let data = match continuation {
                    CommandContinuationRequest::Basic(_) => None,
                    CommandContinuationRequest::Base64(data) => Some(data),
                };

                match state {
                    Some(State::Finished(_)) => {
                        warn!("unexpected event");

                        // TODO: Remove `await`
                        client
                            .enqueue_authenticate_data(AuthenticateData::Cancel)
                            .await;
                    }
                    _ => {
                        // Feed the data into the sasl exchange.
                        let mut out = Vec::new();
                        // TODO: `step` panics after `Err(_)`?
                        match sasl_session.step(data.as_deref(), &mut out) {
                            Ok(new_state) => {
                                info!(?new_state);
                                // TODO: Remove `await`
                                client
                                    .enqueue_authenticate_data(AuthenticateData::Continue(
                                        Secret::new(out),
                                    ))
                                    .await;
                                state = Some(new_state);
                            }
                            Err(error) => {
                                error!(?error);
                                // TODO: Remove `await`
                                client
                                    .enqueue_authenticate_data(AuthenticateData::Cancel)
                                    .await;
                            }
                        }
                    }
                }
            }
            ClientFlowEvent::StatusReceived {
                status: Status::Tagged(Tagged { tag: got_tag, .. }),
            } if got_tag == tag => {
                // **Important**: We MUST ensure the SASL exchange was fully completed!
                // Otherwise, we can't uphold the security guarantees the different mechanisms
                // offer, such as mutual authentication.
                match state {
                    Some(State::Finished(_)) => info!("Finished"),
                    _ => error!("Aborted"),
                }

                break;
            }
            _ => {
                warn!("unexpected event");
            }
        }
    }
}
