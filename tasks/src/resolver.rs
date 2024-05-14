use imap_flow::{client::ClientFlow, Flow, FlowInterrupt};
use imap_types::response::{Response, Status};
use tracing::warn;

use crate::{Scheduler, SchedulerError, SchedulerEvent, Task, TaskHandle};

/// The resolver is a scheduler than manages one task at a time.
pub struct Resolver {
    scheduler: Scheduler,
}

impl Flow for Resolver {
    type Event = SchedulerEvent;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.scheduler.enqueue_input(bytes);
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        self.scheduler.progress()
    }
}

impl Resolver {
    /// Create a new resolver.
    pub fn new(flow: ClientFlow) -> Self {
        Self {
            scheduler: Scheduler::new(flow),
        }
    }

    /// Enqueue a [`Task`] for immediate resolution.
    pub fn resolve<T: Task>(&mut self, task: T) -> ResolvingTask<T> {
        let handle = self.scheduler.enqueue_task(task);

        ResolvingTask {
            resolver: self,
            handle,
        }
    }
}

pub struct ResolvingTask<'a, T: Task> {
    resolver: &'a mut Resolver,
    handle: TaskHandle<T>,
}

impl<T: Task> Flow for ResolvingTask<'_, T> {
    type Event = T::Output;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.resolver.enqueue_input(bytes);
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        loop {
            match self.resolver.progress()? {
                SchedulerEvent::TaskFinished(mut token) => {
                    if let Some(output) = self.handle.resolve(&mut token) {
                        break Ok(output);
                    } else {
                        warn!(?token, "received unexpected task token")
                    }
                }
                SchedulerEvent::Unsolicited(unsolicited) => {
                    if let Response::Status(Status::Bye(bye)) = unsolicited {
                        let err = SchedulerError::UnexpectedByeResponse(bye);
                        break Err(FlowInterrupt::Error(err));
                    } else {
                        warn!(?unsolicited, "received unsolicited");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::Resolver;

    assert_impl_all!(Resolver: Send);
}
