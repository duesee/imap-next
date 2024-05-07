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

pub struct Resolver {
    scheduler: Scheduler,
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
                        break Err(FlowInterrupt::Error(SchedulerError::UnexpectedByeResponse(
                            bye,
                        )));
                    } else {
                        println!("unsolicited: {unsolicited:?}");
                    }
                }
            }
        }
    }
}

impl Resolver {
    pub fn new(scheduler: Scheduler) -> Self {
        Self {
            scheduler,
            handle: None,
        }
    }

    fn enqueue_task<T: Task>(&mut self, task: T) -> TaskHandle<T> {
        let handle = self.scheduler.enqueue_task(task);
        self.handle = Some(handle.handle.clone());
        handle
    }

    pub fn capability(&mut self) -> TaskHandle<CapabilityTask> {
        self.enqueue_task(CapabilityTask::new())
    }

    pub fn authenticate_plain(
        &mut self,
        login: &str,
        passwd: &str,
    ) -> TaskHandle<AuthenticateTask> {
        self.enqueue_task(AuthenticateTask::new_plain(login, passwd, true))
    }

    pub fn logout(&mut self) -> TaskHandle<LogoutTask> {
        self.enqueue_task(LogoutTask::new())
    }

    pub fn noop(&mut self) -> TaskHandle<NoOpTask> {
        self.enqueue_task(NoOpTask::new())
    }
}

pub struct TaskResolved(Box<dyn Any + Send>);

impl TaskResolved {
    pub fn resolve<T: Task>(self, _phantom: TaskHandle<T>) -> T::Output {
        *self.0.downcast::<T::Output>().unwrap()
    }
}
