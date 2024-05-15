use imap_types::{
    command::CommandBody,
    mailbox::Mailbox,
    response::{StatusBody, StatusKind},
    sequence::SequenceSet,
};

use super::TaskError;
use crate::Task;

pub struct CopyTask {
    sequence_set: SequenceSet,
    mailbox: Mailbox<'static>,
    uid: bool,
}

impl Task for CopyTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Copy {
            sequence_set: self.sequence_set.clone(),
            mailbox: self.mailbox.clone(),
            uid: self.uid,
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl CopyTask {
    pub fn new(sequence_set: SequenceSet, mailbox: Mailbox<'static>) -> Self {
        Self {
            sequence_set,
            mailbox,
            uid: true,
        }
    }

    pub fn set_uid(&mut self, uid: bool) {
        self.uid = uid;
    }

    pub fn with_uid(mut self, uid: bool) -> Self {
        self.set_uid(uid);
        self
    }
}
