use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    core::{Charset, Vec1},
    extensions::sort::SortCriterion,
    response::{Data, StatusBody, StatusKind},
    search::SearchKey,
};

use crate::{SchedulerError, Task};

pub type SortTaskOutput = Vec<NonZeroU32>;

#[derive(Clone, Debug)]
pub struct SortTask {
    sort_criteria: Vec1<SortCriterion>,
    charset: Charset<'static>,
    search_criteria: Vec1<SearchKey<'static>>,
    uid: bool,
    output: SortTaskOutput,
}

impl SortTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        sort_criteria: Vec1<SortCriterion>,
        charset: Charset<'static>,
        search_criteria: Vec1<SearchKey<'static>>,
        uid: bool,
    ) -> Self {
        Self {
            sort_criteria,
            charset,
            search_criteria,
            uid,
            output: Default::default(),
        }
    }
}

impl Task for SortTask {
    type Output = Result<SortTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Sort {
            sort_criteria: self.sort_criteria.clone(),
            charset: self.charset.clone(),
            search_criteria: self.search_criteria.clone(),
            uid: self.uid,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Sort(ids) = data {
            self.output = ids;
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
