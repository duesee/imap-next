use std::any::Any;

use imap_flow::{client::ClientFlowCommandHandle, Flow, FlowInterrupt};
use imap_types::response::{Response, Status};

use crate::{
    tasks::{
        authenticate::AuthenticateTask, capability::CapabilityTask, logout::LogoutTask,
        noop::NoOpTask,
    },
    Scheduler, SchedulerError, SchedulerEvent, Task, TaskHandle,
};

/// Resolver managing tasks in serie, based on [`Scheduler`].
///
/// The aim of this flow is to simplify the code needed to execute one
/// single task. For scheduling multiple tasks, use the [`Scheduler`]
/// directly instead.
pub struct Resolver {
    scheduler: Scheduler,
    /// The task handle of the current command being processed.
    handle: Option<ClientFlowCommandHandle>,
}

impl Flow for Resolver {
    type Event = TaskResolved;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.scheduler.enqueue_input(bytes);
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        loop {
            match self.scheduler.progress()? {
                SchedulerEvent::TaskFinished(mut token) => {
                    if let Some(handle) = self.handle.take() {
                        if token.handle == handle {
                            if let Some(output) = token.output.take() {
                                break Ok(TaskResolved(output));
                            }
                        }
                    }
                }
                SchedulerEvent::Unsolicited(unsolicited) => {
                    if let Response::Status(Status::Bye(bye)) = unsolicited {
                        let err = SchedulerError::UnexpectedByeResponse(bye);
                        break Err(FlowInterrupt::Error(err));
                    } else {
                        println!("unsolicited: {unsolicited:?}");
                    }
                }
            }
        }
    }
}

impl Resolver {
    /// Creates a new resolver from a [`Scheduler`].
    pub fn new(scheduler: Scheduler) -> Self {
        Self {
            scheduler,
            handle: None,
        }
    }

    /// Enqueues the given task to the [`Scheduler`], then holds its
    /// handle for later resolution.
    fn enqueue_task<T: Task>(&mut self, task: T) -> TaskHandle<T> {
        let handle = self.scheduler.enqueue_task(task);
        self.handle = Some(handle.handle.clone());
        handle
    }

    /// Enqueues the [`CapabilityTask`].
    pub fn capability(&mut self) -> TaskHandle<CapabilityTask> {
        self.enqueue_task(CapabilityTask::new())
    }

    /// Enqueues the [`AuthenticateTask`], using the `PLAIN` auth
    /// mechanism.
    pub fn authenticate_plain(
        &mut self,
        login: &str,
        passwd: &str,
    ) -> TaskHandle<AuthenticateTask> {
        self.enqueue_task(AuthenticateTask::new_plain(login, passwd, true))
    }

    /// Enqueues the [`LogoutTask`].
    pub fn logout(&mut self) -> TaskHandle<LogoutTask> {
        self.enqueue_task(LogoutTask::new())
    }

    /// Enqueues the [`NoOpTask`].
    pub fn noop(&mut self) -> TaskHandle<NoOpTask> {
        self.enqueue_task(NoOpTask::new())
    }
}

/// Task resolved event.
pub struct TaskResolved(Box<dyn Any + Send>);

impl TaskResolved {
    /// Resolves the task output by the given task handle.
    ///
    /// The [`TaskHandle`] is used as phantom data, in order to
    /// properly downcast the output.
    pub fn resolve<T: Task>(self, _phantom: TaskHandle<T>) -> T::Output {
        *self.0.downcast::<T::Output>().unwrap()
    }
}
