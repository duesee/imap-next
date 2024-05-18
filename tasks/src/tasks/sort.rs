use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    core::{Charset, Vec1},
    extensions::sort::SortCriterion,
    response::{Data, StatusBody, StatusKind},
    search::SearchKey,
};
use tracing::warn;

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct SortTask {
    sort_criteria: Vec1<SortCriterion>,
    charset: Charset<'static>,
    search_criteria: Vec1<SearchKey<'static>>,
    uid: bool,
    output: Option<Vec<NonZeroU32>>,
}

impl SortTask {
    pub fn new(
        sort_criteria: Vec1<SortCriterion>,
        search_criteria: Vec1<SearchKey<'static>>,
    ) -> Self {
        Self {
            sort_criteria,
            charset: Charset::try_from("UTF-8").unwrap(),
            search_criteria,
            uid: true,
            output: Default::default(),
        }
    }

    pub fn set_charset(&mut self, charset: Charset<'static>) {
        self.charset = charset;
    }

    pub fn with_charset(mut self, charset: Charset<'static>) -> Self {
        self.set_charset(charset);
        self
    }

    pub fn set_uid(&mut self, uid: bool) {
        self.uid = uid;
    }

    pub fn with_uid(mut self, uid: bool) -> Self {
        self.set_uid(uid);
        self
    }
}

impl Task for SortTask {
    type Output = Result<Vec<NonZeroU32>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Sort {
            sort_criteria: self.sort_criteria.clone(),
            charset: self.charset.clone(),
            search_criteria: self.search_criteria.clone(),
            uid: self.uid,
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Sort(ids) = data {
            if self.output.is_some() {
                warn!("received duplicate sort data");
            }
            self.output = Some(ids);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => match self.output {
                Some(output) => Ok(output),
                None => Err(TaskError::MissingData("SORT".into())),
            },
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}
