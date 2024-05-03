use imap_types::{
    command::CommandBody,
    mailbox::Mailbox,
    response::{StatusBody, StatusKind},
    sequence::SequenceSet,
};

use crate::{SchedulerError, Task};

pub type MoveTaskOutput = ();

#[derive(Clone, Debug)]
pub struct MoveTask {
    sequence_set: SequenceSet,
    mailbox: Mailbox<'static>,
    uid: bool,
}

impl MoveTask {
    pub fn new(sequence_set: SequenceSet, mailbox: Mailbox<'static>, uid: bool) -> Self {
        Self {
            sequence_set,
            mailbox,
            uid,
        }
    }
}

impl Task for MoveTask {
    type Output = Result<MoveTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Move {
            sequence_set: self.sequence_set.clone(),
            mailbox: self.mailbox.clone(),
            uid: self.uid,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
