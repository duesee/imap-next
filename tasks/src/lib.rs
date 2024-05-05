pub mod tasks;

use std::{
    any::Any,
    collections::VecDeque,
    fmt::{Debug, Formatter},
    marker::PhantomData,
};

use imap_flow::{
    client::{ClientFlow, ClientFlowCommandHandle, ClientFlowError, ClientFlowEvent},
    Flow, FlowInterrupt,
};
use imap_types::{
    auth::AuthenticateData,
    command::{Command, CommandBody},
    core::Tag,
    response::{Bye, CommandContinuationRequest, Data, Response, Status, StatusBody, Tagged},
};
use tag_generator::TagGenerator;
use tasks::{
    authenticate::AuthenticateTask, capability::CapabilityTask, logout::LogoutTask, noop::NoOpTask,
};
use thiserror::Error;

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

    /// Process untagged response.
    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        // Default: Don't process untagged status
        Some(status_body)
    }

    /// Process command continuation request response.
    fn process_continuation_request(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>> {
        // Default: Don't process command continuation request response
        Some(continuation)
    }

    /// Process command continuation request response (during authenticate).
    fn process_continuation_request_authenticate(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData, CommandContinuationRequest<'static>> {
        // Default: Don't process command continuation request response (during authenticate)
        Err(continuation)
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
    waiting_tasks: TaskMap,
    active_tasks: TaskMap,
    tag_generator: TagGenerator,
}

impl Flow for Scheduler {
    type Event = SchedulerEvent;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.enqueue_input(bytes);
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        self.progress()
    }
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
        let tag = self.tag_generator.generate();

        let cmd = {
            let body = task.command_body();
            Command {
                tag: tag.clone(),
                body,
            }
        };

        let handle = self.flow.enqueue_command(cmd);

        self.waiting_tasks.push_back(handle, tag, Box::new(task));

        TaskHandle::new(handle)
    }

    /// Enqueue a [`Task`] for immediate resolution.
    pub fn run_task<'a, T: Task>(&'a mut self, task: T) -> TaskRunner<'a, T> {
        let handle = self.enqueue_task(task);

        TaskRunner {
            scheduler: self,
            handle,
        }
    }

    /// Run the [`CapabilityTask`].
    pub fn capability(&mut self) -> TaskRunner<CapabilityTask> {
        self.run_task(CapabilityTask::new())
    }

    /// Run the [`AuthenticateTask`], using the `PLAIN` auth mechanism.
    pub fn authenticate_plain(
        &mut self,
        login: &str,
        passwd: &str,
    ) -> TaskRunner<AuthenticateTask> {
        self.run_task(AuthenticateTask::plain(login, passwd, true))
    }

    /// Run the [`LogoutTask`].
    pub fn logout(&mut self) -> TaskRunner<LogoutTask> {
        self.run_task(LogoutTask::new())
    }

    /// Run the [`NoOpTask`].
    pub fn noop(&mut self) -> TaskRunner<NoOpTask> {
        self.run_task(NoOpTask::new())
    }

    pub fn enqueue_input(&mut self, bytes: &[u8]) {
        self.flow.enqueue_input(bytes);
    }

    /// Progress the connection returning the next event.
    pub fn progress(&mut self) -> Result<SchedulerEvent, FlowInterrupt<SchedulerError>> {
        loop {
            let event = match self.flow.progress() {
                Ok(event) => event,
                Err(FlowInterrupt::Io(io)) => return Err(FlowInterrupt::Io(io)),
                Err(FlowInterrupt::Error(err)) => {
                    return Err(FlowInterrupt::Error(SchedulerError::Flow(err)))
                }
            };

            match event {
                ClientFlowEvent::GreetingReceived { .. } => {
                    // We have no use for this yet
                }
                ClientFlowEvent::CommandSent { handle, .. } => {
                    // This `unwrap` can't fail because `waiting_tasks` contains all unsent `Commands`.
                    let (handle, tag, task) = self.waiting_tasks.remove_by_handle(handle).unwrap();
                    self.active_tasks.push_back(handle, tag, task);
                }
                ClientFlowEvent::CommandRejected { handle, status, .. } => {
                    let body = match status {
                        Status::Tagged(Tagged { body, .. }) => body,
                        _ => unreachable!(),
                    };

                    // This `unwrap` can't fail because `active_tasks` contains all in-progress `Commands`.
                    let (_, _, task) = self.active_tasks.remove_by_handle(handle).unwrap();

                    let output = Some(task.process_tagged(body));

                    return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                }
                ClientFlowEvent::AuthenticateStarted { handle } => {
                    let (handle, tag, task) = self.waiting_tasks.remove_by_handle(handle).unwrap();
                    self.active_tasks.push_back(handle, tag, task);
                }
                ClientFlowEvent::AuthenticateContinuationRequestReceived {
                    handle,
                    continuation_request,
                } => {
                    let task = self.active_tasks.get_task_by_handle_mut(handle).unwrap();
                    if let Err(continuation) =
                        task.process_continuation_request_authenticate(continuation_request)
                    {
                        return Ok(SchedulerEvent::Unsolicited(
                            Response::CommandContinuationRequest(continuation),
                        ));
                    }
                }
                ClientFlowEvent::AuthenticateStatusReceived { handle, status, .. } => {
                    let (_, _, task) = self.active_tasks.remove_by_handle(handle).unwrap();

                    let body = match status {
                        Status::Untagged(_) => unreachable!(),
                        Status::Tagged(tagged) => tagged.body,
                        Status::Bye(_) => unreachable!(),
                    };

                    let output = Some(task.process_tagged(body));

                    return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                }
                ClientFlowEvent::DataReceived { data } => {
                    if let Some(data) =
                        trickle_down(data, self.active_tasks.tasks_mut(), |task, data| {
                            task.process_data(data)
                        })
                    {
                        return Ok(SchedulerEvent::Unsolicited(Response::Data(data)));
                    }
                }
                ClientFlowEvent::ContinuationRequestReceived {
                    continuation_request,
                } => {
                    if let Some(continuation) = trickle_down(
                        continuation_request,
                        self.active_tasks.tasks_mut(),
                        |task, continuation_request| {
                            task.process_continuation_request(continuation_request)
                        },
                    ) {
                        return Ok(SchedulerEvent::Unsolicited(
                            Response::CommandContinuationRequest(continuation),
                        ));
                    }
                }
                ClientFlowEvent::StatusReceived { status } => match status {
                    Status::Untagged(body) => {
                        if let Some(body) =
                            trickle_down(body, self.active_tasks.tasks_mut(), |task, body| {
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
                            trickle_down(bye, self.active_tasks.tasks_mut(), |task, bye| {
                                task.process_bye(bye)
                            })
                        {
                            return Ok(SchedulerEvent::Unsolicited(Response::Status(Status::Bye(
                                bye,
                            ))));
                        }
                    }
                    Status::Tagged(Tagged { tag, body }) => {
                        let Some((handle, _, task)) = self.active_tasks.remove_by_tag(&tag) else {
                            return Err(FlowInterrupt::Error(
                                SchedulerError::UnexpectedTaggedResponse(Tagged { tag, body }),
                            ));
                        };

                        let output = Some(task.process_tagged(body));

                        return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                    }
                },
                ClientFlowEvent::IdleCommandSent { .. } => todo!(),
                ClientFlowEvent::IdleAccepted { .. } => todo!(),
                ClientFlowEvent::IdleRejected { .. } => todo!(),
                ClientFlowEvent::IdleDoneSent { .. } => todo!(),
            }
        }
    }
}

#[derive(Default)]
struct TaskMap {
    tasks: VecDeque<(ClientFlowCommandHandle, Tag<'static>, Box<dyn TaskAny>)>,
}

impl TaskMap {
    fn push_back(
        &mut self,
        handle: ClientFlowCommandHandle,
        tag: Tag<'static>,
        task: Box<dyn TaskAny>,
    ) {
        self.tasks.push_back((handle, tag, task));
    }

    fn get_task_by_handle_mut(
        &mut self,
        handle: ClientFlowCommandHandle,
    ) -> Option<&mut Box<dyn TaskAny>> {
        self.tasks
            .iter_mut()
            .find_map(|(current_handle, _, task)| (handle == *current_handle).then_some(task))
    }

    fn tasks_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn TaskAny>> {
        self.tasks.iter_mut().map(|(_, _, task)| task)
    }

    fn remove_by_handle(
        &mut self,
        handle: ClientFlowCommandHandle,
    ) -> Option<(ClientFlowCommandHandle, Tag<'static>, Box<dyn TaskAny>)> {
        let index = self
            .tasks
            .iter()
            .position(|(current_handle, _, _)| handle == *current_handle)?;
        self.tasks.remove(index)
    }

    fn remove_by_tag(
        &mut self,
        tag: &Tag,
    ) -> Option<(ClientFlowCommandHandle, Tag<'static>, Box<dyn TaskAny>)> {
        let index = self
            .tasks
            .iter()
            .position(|(_, current_tag, _)| tag == current_tag)?;
        self.tasks.remove(index)
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

    #[error("unexpected BYE response")]
    UnexpectedByeResponse(Bye<'static>),
}

pub struct TaskRunner<'a, T: Task> {
    scheduler: &'a mut Scheduler,
    handle: TaskHandle<T>,
}

impl<T: Task> Flow for TaskRunner<'_, T> {
    type Event = T::Output;
    type Error = SchedulerError;

    fn enqueue_input(&mut self, bytes: &[u8]) {
        self.scheduler.enqueue_input(bytes);
    }

    fn progress(&mut self) -> Result<Self::Event, FlowInterrupt<Self::Error>> {
        loop {
            match self.scheduler.progress()? {
                SchedulerEvent::TaskFinished(mut token) => {
                    if let Some(output) = self.handle.resolve(&mut token) {
                        break Ok(output);
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

    fn process_continuation_request(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>>;

    fn process_continuation_request_authenticate(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData, CommandContinuationRequest<'static>>;

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

    fn process_continuation_request(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>> {
        T::process_continuation_request(self, continuation_request)
    }

    fn process_continuation_request_authenticate(
        &mut self,
        continuation_request: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData, CommandContinuationRequest<'static>> {
        T::process_continuation_request_authenticate(self, continuation_request)
    }

    fn process_bye(&mut self, bye: Bye<'static>) -> Option<Bye<'static>> {
        T::process_bye(self, bye)
    }

    /// Returns [`Any`] instead of [`Task::Output`].
    fn process_tagged(self: Box<Self>, status_body: StatusBody<'static>) -> Box<dyn Any> {
        Box::new(T::process_tagged(*self, status_body))
    }
}
