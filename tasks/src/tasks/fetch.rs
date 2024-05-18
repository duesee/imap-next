use std::{collections::HashMap, num::NonZeroU32};

use imap_types::{
    command::CommandBody,
    core::Vec1,
    fetch::{MacroOrMessageDataItemNames, MessageDataItem},
    response::{Data, StatusBody, StatusKind},
    sequence::{SeqOrUid, SequenceSet},
};
use tracing::warn;

use super::TaskError;
use crate::Task;

/// Fetch message data items matching the given sequence set and the
/// given item names (or macro).
#[derive(Clone, Debug)]
pub struct FetchTask {
    sequence_set: SequenceSet,
    macro_or_item_names: MacroOrMessageDataItemNames<'static>,
    uid: bool,
    output: HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>,
}

impl FetchTask {
    pub fn new(sequence_set: SequenceSet, items: MacroOrMessageDataItemNames<'static>) -> Self {
        Self {
            sequence_set,
            macro_or_item_names: items,
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
}

impl Task for FetchTask {
    type Output = Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Fetch {
            sequence_set: self.sequence_set.clone(),
            macro_or_item_names: self.macro_or_item_names.clone(),
            uid: self.uid,
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Fetch { items, seq } = data {
            if let Some(items) = self.output.insert(seq, items) {
                warn!(seq, ?items, "received duplicate items");
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

/// Same as [`FetchTask`], except that it only collects message data
/// items for the first message matching the given id.
#[derive(Clone, Debug)]
pub struct FetchFirstTask {
    id: NonZeroU32,
    macro_or_item_names: MacroOrMessageDataItemNames<'static>,
    uid: bool,
    output: Option<Vec1<MessageDataItem<'static>>>,
}

impl FetchFirstTask {
    pub fn new(id: NonZeroU32, items: MacroOrMessageDataItemNames<'static>) -> Self {
        Self {
            id,
            macro_or_item_names: items,
            uid: true,
            output: None,
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

impl Task for FetchFirstTask {
    type Output = Result<Vec1<MessageDataItem<'static>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Fetch {
            sequence_set: SequenceSet::from(SeqOrUid::from(self.id)),
            macro_or_item_names: self.macro_or_item_names.clone(),
            uid: self.uid,
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Fetch { items, .. } = data {
            self.output = Some(items);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => match self.output {
                Some(items) => Ok(items),
                None => Err(TaskError::MissingData("FETCH: items".into())),
            },
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
