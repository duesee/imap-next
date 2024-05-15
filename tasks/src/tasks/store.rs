use std::{collections::HashMap, num::NonZeroU32};

use imap_types::{
    command::CommandBody,
    core::Vec1,
    fetch::MessageDataItem,
    flag::{Flag, StoreResponse, StoreType},
    response::{Data, StatusBody, StatusKind},
    sequence::SequenceSet,
};
use tracing::warn;

use super::TaskError;
use crate::Task;

/// Alter message data.
#[derive(Clone, Debug)]
pub struct StoreTask {
    sequence_set: SequenceSet,
    kind: StoreType,
    flags: Vec<Flag<'static>>,
    uid: bool,
    output: HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>,
}

impl Task for StoreTask {
    type Output = Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Store {
            sequence_set: self.sequence_set.clone(),
            kind: self.kind.clone(),
            response: StoreResponse::Answer,
            flags: self.flags.clone(),
            uid: self.uid,
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Fetch { items, seq } = data {
            if let Some(items) = self.output.insert(seq, items) {
                warn!(?items, "received duplicate items for message {seq}");
            }

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

impl StoreTask {
    pub fn new(sequence_set: SequenceSet, kind: StoreType, flags: Vec<Flag<'static>>) -> Self {
        Self {
            sequence_set,
            kind,
            flags,
            uid: true,
            output: Default::default(),
        }
    }

    pub fn set_uid(&mut self, uid: bool) {
        self.uid = uid;
    }

    pub fn with_uid(mut self, uid: bool) -> Self {
        self.set_uid(uid);
        self
    }

    pub fn silent(self) -> SilentStoreTask {
        SilentStoreTask::new(self)
    }
}

/// Alter message data instructing the server to not send the updated values.
///
/// Note: Same as [`StoreTask`], except that it does not return any output.
#[derive(Clone, Debug)]
pub struct SilentStoreTask(StoreTask);

impl SilentStoreTask {
    pub fn new(store: StoreTask) -> Self {
        Self(store)
    }
}

impl Task for SilentStoreTask {
    type Output = Result<(), TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Store {
            sequence_set: self.0.sequence_set.clone(),
            kind: self.0.kind.clone(),
            response: StoreResponse::Silent,
            flags: self.0.flags.clone(),
            uid: self.0.uid,
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
