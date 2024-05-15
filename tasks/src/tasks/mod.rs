use imap_types::response::StatusBody;
use thiserror::Error;

pub mod authenticate;
pub mod capability;
pub mod fetch;
pub mod id;
pub mod logout;
pub mod noop;
pub mod search;
pub mod sort;
pub mod starttls;
pub mod store;
pub mod thread;

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("unexpected BAD response: {}", .0.text)]
    UnexpectedBadResponse(StatusBody<'static>),

    #[error("unexpected NO response: {}", .0.text)]
    UnexpectedNoResponse(StatusBody<'static>),

    #[error("missing required data for command {0}")]
    MissingData(String),
}
