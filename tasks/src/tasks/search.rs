use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    core::Vec1,
    response::{Data, StatusBody, StatusKind},
    search::SearchKey,
};

use crate::{SchedulerError, Task};

pub type SearchTaskOutput = Vec<NonZeroU32>;

#[derive(Clone, Debug)]
pub struct SearchTask {
    criteria: Vec1<SearchKey<'static>>,
    uid: bool,
    output: SearchTaskOutput,
}

impl SearchTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(criteria: Vec1<SearchKey<'static>>, uid: bool) -> Self {
        Self {
            criteria,
            uid,
            output: Default::default(),
        }
    }
}

impl Task for SearchTask {
    type Output = Result<SearchTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Search {
            charset: None,
            criteria: self.criteria.clone(),
            uid: self.uid,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Search(ids) = data {
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
