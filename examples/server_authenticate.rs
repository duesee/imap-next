use argh::FromArgs;
use imap_codec::imap_types::{
    auth::AuthMechanism,
    command::CommandBody,
    response::{CommandContinuationRequest, Greeting, Status},
};
use imap_flow::{
    server::{ServerFlow, ServerFlowEvent, ServerFlowOptions},
    stream::AnyStream,
};
use rsasl::{
    callback::{Context, SessionCallback, SessionData},
    prelude::*,
    property::{AuthId, AuthzId, Password},
    validate::{Validate, ValidationError},
};
use tokio::net::TcpListener;
use tracing::{info, info_span, Level};

#[derive(Debug, FromArgs)]
/// Server (Authenticate).
struct Arguments {
    /// host
    #[argh(positional)]
    host: String,

    /// port
    #[argh(positional)]
    port: u16,
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

    let stream = {
        let listener = TcpListener::bind((args.host, args.port)).await.unwrap();

        let (stream, _) = listener.accept().await.unwrap();

        stream
    };

    let greeting = Greeting::ok(None, "Hello, World!").unwrap();

    let (mut server, greeting) = ServerFlow::send_greeting(
        AnyStream::new(stream),
        ServerFlowOptions::default(),
        greeting,
    )
    .await
    .unwrap();

    info!(?greeting);

    let mut state = None;

    loop {
        let span = info_span!("loop", ?state);
        let _enter = span.enter();

        let event = server.progress().await.unwrap();
        info!(?event);

        match event {
            ServerFlowEvent::CommandReceived { command } => {
                if let CommandBody::Authenticate {
                    mechanism,
                    initial_response,
                } = &command.body
                {
                    let mut sasl_session = {
                        let sasl: SASLServer<MyValidation> = SASLServer::new(
                            SASLConfig::builder()
                                .with_defaults()
                                .with_callback(Callback {})
                                .unwrap(),
                        );

                        sasl.start_suggested(
                            Mechname::parse(mechanism.to_string().to_uppercase().as_bytes())
                                .unwrap(),
                        )
                        .unwrap()
                    };

                    let chosen_mechanism =
                        AuthMechanism::try_from(sasl_session.get_mechname().to_string()).unwrap();

                    info!(?chosen_mechanism);

                    let mut out = Vec::new();
                    if let Some(initial_response) = initial_response {
                        state =
                            Some(sasl_session.step(Some(initial_response.declassify()), &mut out));
                    }

                    if let Some(Ok(State::Running)) = state {
                        server.enqueue_continuation(CommandContinuationRequest::base64(out));
                    }

                    if let Some(Ok(State::Finished(_))) = state {
                        let val = sasl_session.validation().unwrap();

                        if val.is_ok() {
                            server.enqueue_status(
                                Status::ok(Some(command.tag), None, "Authentication successful")
                                    .unwrap(),
                            );
                        } else {
                            server.enqueue_status(
                                Status::no(Some(command.tag), None, "Authentication failed")
                                    .unwrap(),
                            );
                        }
                    }

                    // TODO
                }
            }
            // TODO
            // ServerFlowEvent::CommandContinuationReceived {} => {}
            ServerFlowEvent::ResponseSent { .. } => { /*Expected*/ }
        }
    }
}

struct Callback;

impl Callback {
    fn validate_plain(&self, context: &Context) -> Result<(), ValidationError> {
        let authzid = context
            .get_ref::<AuthzId>()
            .ok_or(ValidationError::Boxed("no authzid".into()))?;
        let authid = context
            .get_ref::<AuthId>()
            .ok_or(ValidationError::Boxed("no authid".into()))?;
        let password = context
            .get_ref::<Password>()
            .ok_or(ValidationError::Boxed("no password".into()))?;

        info!(?authzid, ?authid, ?password,);

        if authid == "alice" && password == b"password" {
            Ok(())
        } else {
            Err(ValidationError::Boxed("invalid credentials".into()))
        }
    }
}

impl SessionCallback for Callback {
    fn validate(
        &self,
        session_data: &SessionData,
        context: &Context,
        _: &mut Validate<'_>,
    ) -> Result<(), ValidationError> {
        match session_data.mechanism().mechanism.as_str() {
            "PLAIN" => self.validate_plain(context),
            _ => Err(ValidationError::Boxed(
                "unsupported authentication mechanism".into(),
            )),
        }
    }
}

struct MyValidation;

impl Validation for MyValidation {
    type Value = Result<(), ValidationError>;
}
