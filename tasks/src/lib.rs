use std::{
    any::Any,
    collections::HashMap,
    fmt::{Debug, Formatter},
    marker::PhantomData,
};

use imap_codec::imap_types::{
    command::{Command, CommandBody},
    core::Tag,
    response::{Bye, CommandContinuationRequest, Data, Response, Status, StatusBody, Tagged},
};
use imap_flow::client::{ClientFlow, ClientFlowCommandHandle, ClientFlowError, ClientFlowEvent};
use tag_generator::TagGenerator;
use thiserror::Error;

pub mod tasks;

/// Tells how a specific IMAP [`Command`] is processed.
///
/// Most `process_` trait methods consume interesting responses (returning `None`),
/// and move out uninteresting responses (returning `Some(...)`).
///
/// If no active task is interested in a given response, we call this response "unsolicited".
pub trait Task: 'static {
    /// Output of the task.
    ///
    /// Returned in [`Self::process_tagged`].
    type Output;

    /// Returns the [`CommandBody`] to issue for this task.
    ///
    /// Note: The [`Scheduler`] will tag the [`CommandBody`] creating a complete [`Command`].
    fn command_body(&self) -> CommandBody<'static>;

    /// Process data response.
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        // Default: Don't process server data
        Some(data)
    }

    /// Process an untagged response.
    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        // Default: Don't process untagged status
        Some(status_body)
    }

    /// Process an command continuation request response.
    fn process_continuation(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>> {
        // Default: Don't process command continuation request response
        Some(continuation)
    }

    /// Process bye response.
    fn process_bye(&mut self, bye: Bye<'static>) -> Option<Bye<'static>> {
        // Default: Don't process bye
        Some(bye)
    }

    /// Process command completion result response.
    ///
    /// The [`Scheduler`] already chooses the corresponding response by tag.
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output;
}

/// Scheduler managing enqueued tasks and routing incoming responses to active tasks.
pub struct Scheduler {
    flow: ClientFlow,
    waiting_tasks: HashMap<ClientFlowCommandHandle, Box<dyn TaskAny>>,
    active_tasks: HashMap<ClientFlowCommandHandle, (Tag<'static>, Box<dyn TaskAny>)>,
    tag_generator: TagGenerator,
}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new(flow: ClientFlow) -> Self {
        Self {
            flow,
            waiting_tasks: Default::default(),
            active_tasks: Default::default(),
            tag_generator: TagGenerator::new(),
        }
    }

    /// Enqueue a [`Task`].
    pub fn enqueue_task<T>(&mut self, task: T) -> TaskHandle<T>
    where
        T: Task,
    {
        let cmd = {
            let tag = self.tag_generator.generate();
            let body = task.command_body();

            Command { tag, body }
        };

        let handle = self.flow.enqueue_command(cmd);

        let replaced = self.waiting_tasks.insert(handle, Box::new(task));
        assert!(replaced.is_none());

        TaskHandle::new(handle)
    }

    /// Progress the connection returning the next event.
    pub async fn progress(&mut self) -> Result<SchedulerEvent, SchedulerError> {
        loop {
            let event = self.flow.progress().await?;

            match event {
                ClientFlowEvent::CommandSent { handle, command } => {
                    // This `unwrap` can't fail because `waiting_tasks` contains all unsent `Commands`.
                    let entry = self.waiting_tasks.remove(&handle).unwrap();
                    self.active_tasks.insert(handle, (command.tag, entry));
                }
                ClientFlowEvent::CommandRejected { handle, status, .. } => {
                    let body = match status {
                        Status::Tagged(Tagged { body, .. }) => body,
                        _ => unreachable!(),
                    };

                    // This `unwrap` can't fail because `active_tasks` contains all in-progress `Commands`.
                    let (_, task) = self.active_tasks.remove(&handle).unwrap();

                    let output = Some(task.process_tagged(body));

                    return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                }
                ClientFlowEvent::DataReceived { data } => {
                    if let Some(data) = trickle_down(data, self.active_tasks_mut(), |task, data| {
                        task.process_data(data)
                    }) {
                        return Ok(SchedulerEvent::Unsolicited(Response::Data(data)));
                    }
                }
                ClientFlowEvent::ContinuationReceived { continuation } => {
                    if let Some(continuation) = trickle_down(
                        continuation,
                        self.active_tasks_mut(),
                        |task, continuation| task.process_continuation(continuation),
                    ) {
                        return Ok(SchedulerEvent::Unsolicited(
                            Response::CommandContinuationRequest(continuation),
                        ));
                    }
                }
                ClientFlowEvent::StatusReceived { status } => match status {
                    Status::Untagged(body) => {
                        if let Some(body) =
                            trickle_down(body, self.active_tasks_mut(), |task, body| {
                                task.process_untagged(body)
                            })
                        {
                            return Ok(SchedulerEvent::Unsolicited(Response::Status(
                                Status::Untagged(body),
                            )));
                        }
                    }
                    Status::Bye(bye) => {
                        if let Some(bye) =
                            trickle_down(bye, self.active_tasks_mut(), |task, bye| {
                                task.process_bye(bye)
                            })
                        {
                            return Ok(SchedulerEvent::Unsolicited(Response::Status(Status::Bye(
                                bye,
                            ))));
                        }
                    }
                    Status::Tagged(Tagged { tag, body }) => {
                        let handle = {
                            if let Some((handle, _)) =
                                self.active_tasks.iter().find(|(_, (tag_, _))| tag == *tag_)
                            {
                                *handle
                            } else {
                                return Err(SchedulerError::UnexpectedTaggedResponse(Tagged {
                                    tag,
                                    body,
                                }));
                            }
                        };

                        let (_, task) = self.active_tasks.remove(&handle).unwrap();

                        let output = Some(task.process_tagged(body));

                        return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                    }
                },
            }
        }
    }

    fn active_tasks_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn TaskAny>> {
        self.active_tasks.values_mut().map(|(_, task)| task)
    }
}

#[derive(Debug)]
pub enum SchedulerEvent {
    TaskFinished(TaskToken),
    Unsolicited(Response<'static>),
}

#[derive(Debug, Error)]
pub enum SchedulerError {
    /// Flow error.
    #[error("flow error")]
    Flow(#[from] ClientFlowError),
    /// Unexpected tag in command completion result.
    ///
    /// The scheduler received a tag that cannot be matched to an active command.
    /// This could be due to a severe implementation error in the scheduler,
    /// the server, or anything in-between, really.
    ///
    /// It's better to halt the execution to avoid damage.
    #[error("unexpected tag in command completion result")]
    UnexpectedTaggedResponse(Tagged<'static>),
}

#[derive(Eq)]
pub struct TaskHandle<T: Task> {
    handle: ClientFlowCommandHandle,
    _t: PhantomData<T>,
}

impl<T: Task> Debug for TaskHandle<T> {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.debug_struct("TaskHandle")
            .field("handle", &self.handle)
            .finish()
    }
}

impl<T: Task> Clone for TaskHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Task> Copy for TaskHandle<T> {}

impl<T: Task> PartialEq for TaskHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle
    }
}

impl<T: Task> TaskHandle<T> {
    fn new(handle: ClientFlowCommandHandle) -> Self {
        Self {
            handle,
            _t: Default::default(),
        }
    }

    /// Try resolving the task invalidating the token.
    ///
    /// The token is invalidated iff the return value is `Some`.
    pub fn resolve(&self, token: &mut TaskToken) -> Option<T::Output> {
        if token.handle != self.handle {
            return None;
        }

        let output = token.output.take()?;
        let output = output.downcast::<T::Output>().unwrap();

        Some(*output)
    }
}

#[derive(Debug)]
pub struct TaskToken {
    // TODO(#53): Bind this token to a `Scheduler` instance.
    //            Make sure invariants can't be bypassed by creating a second scheduler.
    handle: ClientFlowCommandHandle,
    output: Option<Box<dyn Any>>,
}

// -------------------------------------------------------------------------------------------------

/// Move `trickle` from consumer to consumer until the first consumer doesn't hand it back.
///
/// If none of the consumers is interested in `trickle`, give it back.
fn trickle_down<T, F, I>(trickle: T, consumers: I, f: F) -> Option<T>
where
    I: Iterator,
    F: Fn(&mut I::Item, T) -> Option<T>,
{
    let mut trickle = Some(trickle);

    for mut consumer in consumers {
        if let Some(trickle_) = trickle {
            trickle = f(&mut consumer, trickle_);

            if trickle.is_none() {
                break;
            }
        }
    }

    trickle
}

// -------------------------------------------------------------------------------------------------

/// Helper trait that ...
///
/// * doesn't have an associated type and uses [`Any`] in [`Self::process_tagged`]
/// * is an object-safe "subset" of [`Task`]
trait TaskAny {
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>>;

    fn process_untagged(&mut self, status_body: StatusBody<'static>)
        -> Option<StatusBody<'static>>;

    fn process_continuation(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>>;

    fn process_bye(&mut self, bye: Bye<'static>) -> Option<Bye<'static>>;

    fn process_tagged(self: Box<Self>, status_body: StatusBody<'static>) -> Box<dyn Any>;
}

impl<T> TaskAny for T
where
    T: Task,
{
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        T::process_data(self, data)
    }

    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        T::process_untagged(self, status_body)
    }

    fn process_continuation(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>> {
        T::process_continuation(self, continuation)
    }

    fn process_bye(&mut self, bye: Bye<'static>) -> Option<Bye<'static>> {
        T::process_bye(self, bye)
    }

    /// Returns [`Any`] instead of [`Task::Output`].
    fn process_tagged(self: Box<Self>, status_body: StatusBody<'static>) -> Box<dyn Any> {
        Box::new(T::process_tagged(*self, status_body))
    }
}
