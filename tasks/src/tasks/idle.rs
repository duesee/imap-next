use imap_types::{
    command::CommandBody,
    response::{StatusBody, StatusKind},
};
use tokio::sync::mpsc;
use tracing::{debug, trace};

use crate::{SchedulerError, Task};

pub type IdleTaskOutput = ();

#[derive(Clone, Debug)]
pub struct IdleTask {
    done: mpsc::Sender<()>,
}

impl IdleTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(done: mpsc::Sender<()>) -> Self {
        Self { done }
    }
}

impl Task for IdleTask {
    type Output = Result<IdleTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Idle
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        if let Err(err) = self.done.try_send(()) {
            debug!("cannot send IMAP IDLE done notification: {err}");
            trace!("{err:#?}");
        }

        match status_body.kind {
            StatusKind::Ok => Ok(()),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
