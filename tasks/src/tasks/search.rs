use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    core::Vec1,
    response::{Data, StatusBody, StatusKind},
    search::SearchKey,
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct SearchTask {
    criteria: Vec1<SearchKey<'static>>,
    uid: bool,
    output: Vec<NonZeroU32>,
}

impl Default for SearchTask {
    fn default() -> Self {
        Self {
            criteria: Vec1::from(SearchKey::All),
            uid: true,
            output: Default::default(),
        }
    }
}

impl Task for SearchTask {
    type Output = Result<Vec<NonZeroU32>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Search {
            charset: None,
            criteria: self.criteria.clone(),
            uid: self.uid,
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Search(ids) = data {
            self.output = ids;
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

impl SearchTask {
    pub fn new(criteria: Vec1<SearchKey<'static>>) -> Self {
        Self {
            criteria,
            ..Default::default()
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
