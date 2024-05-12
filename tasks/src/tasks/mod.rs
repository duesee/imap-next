use imap_types::response::StatusBody;
use thiserror::Error;

pub mod authenticate;
pub mod capability;
pub mod logout;
pub mod noop;

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("unexpected BAD response: {}", .0.text)]
    UnexpectedBadResponse(StatusBody<'static>),

    #[error("unexpected NO response: {}", .0.text)]
    UnexpectedNoResponse(StatusBody<'static>),
}
