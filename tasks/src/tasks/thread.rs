use imap_types::{
    command::CommandBody,
    core::{Charset, Vec1},
    extensions::thread::{Thread, ThreadingAlgorithm},
    response::{Data, StatusBody, StatusKind},
    search::SearchKey,
};
use tracing::warn;

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct ThreadTask {
    algorithm: ThreadingAlgorithm<'static>,
    charset: Charset<'static>,
    search_criteria: Vec1<SearchKey<'static>>,
    uid: bool,
    output: Option<Vec<Thread>>,
}

impl ThreadTask {
    pub fn new(
        algorithm: ThreadingAlgorithm<'static>,
        search_criteria: Vec1<SearchKey<'static>>,
    ) -> Self {
        Self {
            algorithm,
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

impl Task for ThreadTask {
    type Output = Result<Vec<Thread>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Thread {
            algorithm: self.algorithm.clone(),
            charset: self.charset.clone(),
            search_criteria: self.search_criteria.clone(),
            uid: self.uid,
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Thread(threads) = data {
            if self.output.is_some() {
                warn!("received duplicate thread data");
            }
            self.output = Some(threads);
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
