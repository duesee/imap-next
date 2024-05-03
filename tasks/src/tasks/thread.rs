use imap_types::{
    command::CommandBody,
    core::{Charset, Vec1},
    extensions::thread::{Thread, ThreadingAlgorithm},
    response::{Data, StatusBody, StatusKind},
    search::SearchKey,
};

use crate::{SchedulerError, Task};

pub type ThreadTaskOutput = Vec<Thread>;

#[derive(Clone, Debug)]
pub struct ThreadTask {
    algorithm: ThreadingAlgorithm<'static>,
    charset: Charset<'static>,
    search_criteria: Vec1<SearchKey<'static>>,
    uid: bool,
    output: ThreadTaskOutput,
}

impl ThreadTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        algorithm: ThreadingAlgorithm<'static>,
        charset: Charset<'static>,
        search_criteria: Vec1<SearchKey<'static>>,
        uid: bool,
    ) -> Self {
        Self {
            algorithm,
            charset,
            search_criteria,
            uid,
            output: Default::default(),
        }
    }
}

impl Task for ThreadTask {
    type Output = Result<ThreadTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Thread {
            algorithm: self.algorithm.clone(),
            charset: self.charset.clone(),
            search_criteria: self.search_criteria.clone(),
            uid: self.uid,
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Thread(threads) = data {
            self.output = threads;
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
