use imap_types::{
    command::CommandBody,
    response::{Data, StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type ExpungeTaskOutput = usize;

#[derive(Clone, Debug)]
pub struct ExpungeTask {
    count: usize,
}

impl ExpungeTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new() -> Self {
        Self { count: 0 }
    }
}

impl Task for ExpungeTask {
    type Output = Result<ExpungeTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Expunge
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Expunge(_) = data {
            self.count += 1;
            None
        } else {
            Some(data)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.count),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
