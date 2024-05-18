use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    response::{Data, StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

/// Permanently removes messages containing the Deleted flag in their
/// envelope.
///
/// Be aware that the returned vector can contain multiple time the
/// same sequence number, depending on the server implementation
/// style:
///
/// > For example, if the last 5 messages in a 9-message mailbox are
/// expunged, a "lower to higher" server will send five untagged
/// EXPUNGE responses for message sequence number 5, whereas a "higher
/// to lower server" will send successive untagged EXPUNGE responses
/// for message sequence numbers 9, 8, 7, 6, and 5
#[derive(Clone, Debug, Default)]
pub struct ExpungeTask {
    output: Vec<NonZeroU32>,
}

impl ExpungeTask {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Task for ExpungeTask {
    type Output = Result<Vec<NonZeroU32>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Expunge
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Expunge(seq) = data {
            self.output.push(seq);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
