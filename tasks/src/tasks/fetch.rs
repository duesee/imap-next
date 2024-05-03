use std::{collections::HashMap, num::NonZeroU32};

use imap_types::{
    command::CommandBody,
    core::Vec1,
    fetch::{MacroOrMessageDataItemNames, MessageDataItem},
    response::{Data, StatusBody, StatusKind},
    sequence::SequenceSet,
};

use crate::{SchedulerError, Task};

pub type FetchTaskOutput = HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>;

#[derive(Clone, Debug)]
pub struct FetchTask {
    sequence_set: SequenceSet,
    items: MacroOrMessageDataItemNames<'static>,
    uid: bool,
    output: FetchTaskOutput,
}

impl FetchTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'static>,
        uid: bool,
    ) -> Self {
        Self {
            sequence_set,
            items,
            uid,
            output: Default::default(),
        }
    }
}

impl Task for FetchTask {
    type Output = Result<FetchTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Fetch {
            sequence_set: self.sequence_set.clone(),
            macro_or_item_names: self.items.clone(),
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

pub type FetchFirstTaskOutput = Option<Vec1<MessageDataItem<'static>>>;

#[derive(Clone, Debug)]
pub struct FetchFirstTask {
    sequence_set: SequenceSet,
    items: MacroOrMessageDataItemNames<'static>,
    uid: bool,
    output: FetchFirstTaskOutput,
}

impl FetchFirstTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'static>,
        uid: bool,
    ) -> Self {
        Self {
            sequence_set,
            items,
            uid,
            output: Default::default(),
        }
    }
}

impl Task for FetchFirstTask {
    type Output = Result<FetchFirstTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Fetch {
            sequence_set: self.sequence_set.clone(),
            macro_or_item_names: self.items.clone().into(),
            uid: self.uid,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        match data {
            Data::Fetch { items, .. } if self.output.is_none() => {
                self.output = Some(items);
                None
            }
            data => Some(data),
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
