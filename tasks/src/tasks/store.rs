use imap_types::{
    command::CommandBody,
    flag::{Flag, StoreResponse, StoreType},
    response::{Data, StatusBody, StatusKind},
    sequence::SequenceSet,
};

use crate::{SchedulerError, Task};

use super::fetch::FetchTaskOutput;

pub type StoreTaskOutput = FetchTaskOutput;

#[derive(Clone, Debug)]
pub struct StoreTask {
    sequence_set: SequenceSet,
    kind: StoreType,
    flags: Vec<Flag<'static>>,
    uid: bool,
    output: StoreTaskOutput,
}

impl StoreTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: Vec<Flag<'static>>,
        uid: bool,
    ) -> Self {
        Self {
            sequence_set,
            kind,
            flags,
            uid,
            output: Default::default(),
        }
    }
}

impl Task for StoreTask {
    type Output = Result<StoreTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Store {
            sequence_set: self.sequence_set.clone(),
            kind: self.kind.clone(),
            response: StoreResponse::Answer,
            flags: self.flags.clone(),
            uid: self.uid,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Fetch { items, seq } = data {
            self.output.insert(seq, items);
            None
        } else {
            Some(data)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}

pub type SilentStoreTaskOutput = ();

#[derive(Clone, Debug)]
pub struct SilentStoreTask {
    sequence_set: SequenceSet,
    kind: StoreType,
    flags: Vec<Flag<'static>>,
    uid: bool,
}

impl SilentStoreTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: Vec<Flag<'static>>,
        uid: bool,
    ) -> Self {
        Self {
            sequence_set,
            kind,
            flags,
            uid,
        }
    }
}

impl Task for SilentStoreTask {
    type Output = Result<SilentStoreTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Store {
            sequence_set: self.sequence_set.clone(),
            kind: self.kind.clone(),
            response: StoreResponse::Silent,
            flags: self.flags.clone(),
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
