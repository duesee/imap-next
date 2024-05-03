pub mod tasks;

use std::{
    any::Any,
    borrow::Cow,
    cmp::Ordering,
    collections::{HashSet, VecDeque},
    fmt,
    marker::PhantomData,
    slice,
    time::Duration,
};

use imap_flow::{
    client::{
        ClientFlow, ClientFlowCommandHandle, ClientFlowError, ClientFlowEvent, ClientFlowOptions,
    },
    stream::AnyStream,
};
use imap_types::{
    auth::{AuthMechanism, AuthenticateData},
    bounded_static::{IntoBoundedStatic, ToBoundedStatic},
    command::{Command, CommandBody},
    core::{Charset, IString, Literal, LiteralMode, NString, Tag, Vec1},
    error::ValidationError,
    extensions::{
        binary::{Literal8, LiteralOrLiteral8},
        sort::{SortCriterion, SortKey},
        thread::ThreadingAlgorithm,
    },
    fetch::{MacroOrMessageDataItemNames, MessageDataItem, MessageDataItemName},
    flag::{Flag, StoreType},
    mailbox::{ListMailbox, Mailbox},
    response::{
        Bye, Capability, Code, CommandContinuationRequest, Data, Response, Status, StatusBody,
        Tagged,
    },
    search::SearchKey,
    sequence::SequenceSet,
};
use tag_generator::TagGenerator;
use tasks::{
    append::{
        AppendTask, AppendTaskOutput, PostAppendCheckTask, PostAppendCheckTaskOutput,
        PostAppendNoOpTask, PostAppendNoOpTaskOutput,
    },
    appenduid::{AppendUidTask, AppendUidTaskOutput},
    authenticate::AuthenticateTask,
    capability::{CapabilityTask, CapabilityTaskOutput},
    copy::{CopyTask, CopyTaskOutput},
    create::{CreateTask, CreateTaskOutput},
    delete::{DeleteTask, DeleteTaskOutput},
    expunge::{ExpungeTask, ExpungeTaskOutput},
    fetch::{FetchFirstTask, FetchFirstTaskOutput, FetchTask, FetchTaskOutput},
    id::{IdTask, IdTaskOutput},
    idle::{IdleTask, IdleTaskOutput},
    list::{ListTask, ListTaskOutput},
    noop::{NoOpTask, NoOpTaskOutput},
    r#move::{MoveTask, MoveTaskOutput},
    search::{SearchTask, SearchTaskOutput},
    select::{SelectTask, SelectTaskOutput},
    sort::{SortTask, SortTaskOutput},
    store::{SilentStoreTask, SilentStoreTaskOutput, StoreTask, StoreTaskOutput},
    thread::{ThreadTask, ThreadTaskOutput},
};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error};

/// Tells how a specific IMAP [`Command`] is processed.
///
/// Most `process_` trait methods consume interesting responses (returning `None`),
/// and move out uninteresting responses (returning `Some(...)`).
///
/// If no active task is interested in a given response, we call this response "unsolicited".
pub trait Task: Clone + fmt::Debug + Send + 'static {
    /// Output of the task.
    ///
    /// Returned in [`Self::process_tagged`].
    type Output;

    /// Returns the [`CommandBody`] to issue for this task.
    ///
    /// Note: The [`Scheduler`] will tag the [`CommandBody`] creating a complete [`Command`].
    fn command_body(&self) -> CommandBody<'static>;

    /// Process data response.
    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        // Default: Don't process server data
        Some(data)
    }

    /// Process untagged response.
    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_untagged(
        &mut self,
        status_body: StatusBody<'static>,
    ) -> Option<StatusBody<'static>> {
        // Default: Don't process untagged status
        Some(status_body)
    }

    /// Process command continuation request response.
    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_continuation_request(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Option<CommandContinuationRequest<'static>> {
        // Default: Don't process command continuation request response
        Some(continuation)
    }

    /// Process command continuation request response (during authenticate).
    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_continuation_request_authenticate(
        &mut self,
        continuation: CommandContinuationRequest<'static>,
    ) -> Result<AuthenticateData<'static>, CommandContinuationRequest<'static>> {
        // Default: Don't process command continuation request response (during authenticate)
        Err(continuation)
    }

    /// Process bye response.
    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
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
#[derive(Debug)]
pub struct Scheduler {
    flow: ClientFlow,
    waiting_tasks: TaskMap,
    active_tasks: TaskMap,
    tag_generator: TagGenerator,
    capabilities: Vec1<Capability<'static>>,
    server_id: Option<Vec<(IString<'static>, NString<'static>)>>,
}

impl Scheduler {
    pub async fn try_new_flow(
        stream: AnyStream,
        options: ClientFlowOptions,
    ) -> Result<Self, SchedulerError> {
        let (flow, greeting) = ClientFlow::receive_greeting(stream, options).await?;

        let mut scheduler = Self {
            flow,
            waiting_tasks: Default::default(),
            active_tasks: Default::default(),
            tag_generator: TagGenerator::new(),
            capabilities: Vec1::from(Capability::Imap4Rev1),
            server_id: Default::default(),
        };

        // Capabilities may not be returned by the greeting. In this
        // particular case, we explicitly request them.
        scheduler.capabilities = if let Some(Code::Capability(capabilities)) = greeting.code {
            capabilities
        } else {
            scheduler.capability().await?
        };

        Ok(scheduler)
    }

    pub async fn authenticate<'a>(
        &mut self,
        authenticate: AuthenticateTask,
        id_params: Vec<(IString<'a>, NString<'a>)>,
    ) -> Result<(), SchedulerError> {
        let capabilities = self.exec_task(authenticate).await??;

        // Capabilities may change before and after authentication. In
        // order to get the most up-to-date capabilities, we need to
        // request them once again.
        self.capabilities = if let Some(capabilities) = capabilities {
            capabilities
        } else {
            self.capability().await?
        };

        if self.ext_id_supported() {
            self.server_id = self.id(id_params).await?;
        }

        Ok(())
    }

    pub async fn authenticate_plain<'a>(
        &mut self,
        login: &str,
        passwd: &str,
        id_params: Vec<(IString<'a>, NString<'a>)>,
    ) -> Result<(), SchedulerError> {
        let ir = self.ext_sasl_ir_supported();
        self.authenticate(AuthenticateTask::new_plain(login, passwd, ir), id_params)
            .await
    }

    pub async fn authenticate_xoauth2<'a>(
        &mut self,
        user: &str,
        token: &str,
        id_params: Vec<(IString<'a>, NString<'a>)>,
    ) -> Result<(), SchedulerError> {
        let ir = self.ext_sasl_ir_supported();
        self.authenticate(AuthenticateTask::new_xoauth2(user, token, ir), id_params)
            .await
    }

    pub async fn authenticate_oauth_bearer<'a>(
        &mut self,
        a: &str,
        host: &str,
        port: u16,
        token: &str,
        id_params: Vec<(IString<'a>, NString<'a>)>,
    ) -> Result<(), SchedulerError> {
        let ir = self.ext_sasl_ir_supported();
        self.authenticate(
            AuthenticateTask::new_oauth_bearer(a, host, port, token, ir),
            id_params,
        )
        .await
    }

    pub fn set_some_idle_timeout(&mut self, secs: Option<u64>) {
        self.flow.set_some_idle_timeout(secs);
    }

    pub fn set_idle_timeout(&mut self, secs: u64) {
        self.flow.set_idle_timeout(secs);
    }

    pub fn get_idle_timeout(&self) -> Duration {
        self.flow.get_idle_timeout()
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

    pub async fn exec_task<T>(&mut self, task: T) -> Result<T::Output, SchedulerError>
    where
        T: Task,
    {
        let handle = self.enqueue_task(task);

        loop {
            match self.progress().await? {
                SchedulerEvent::TaskFinished(mut token) => {
                    if let Some(output) = handle.resolve(&mut token) {
                        break Ok(output);
                    }
                }
                SchedulerEvent::Unsolicited(unsolicited) => {
                    if let Response::Status(Status::Bye(bye)) = unsolicited {
                        break Err(SchedulerError::UnexpectedByeResponse(bye));
                    } else {
                        debug!("received not handled unsolicited response: {unsolicited:?}");
                    }
                }
            }
        }
    }

    pub fn set_idle_done(&mut self) -> Option<ClientFlowCommandHandle> {
        self.flow.set_idle_done()
    }

    /// Progress the connection returning the next event.
    pub async fn progress(&mut self) -> Result<SchedulerEvent, SchedulerError> {
        loop {
            let event = self.flow.progress().await?;

            match event {
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
                    let continuation =
                        task.process_continuation_request_authenticate(continuation_request);

                    match continuation {
                        Ok(data) => {
                            self.flow.set_authenticate_data(data).unwrap();
                        }
                        Err(continuation) => {
                            return Ok(SchedulerEvent::Unsolicited(
                                Response::CommandContinuationRequest(continuation),
                            ));
                        }
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
                    if self.flow.is_waiting_for_idle_done_set() {
                        self.flow.set_idle_done();
                    }

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
                            return Err(SchedulerError::UnexpectedTaggedResponse(Tagged {
                                tag,
                                body,
                            }));
                        };

                        let output = Some(task.process_tagged(body));

                        return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                    }
                },
                ClientFlowEvent::IdleCommandSent { handle, .. } => {
                    // This `unwrap` can't fail because `waiting_tasks` contains all unsent `Commands`.
                    let (handle, tag, task) = self.waiting_tasks.remove_by_handle(handle).unwrap();
                    self.active_tasks.push_back(handle, tag, task);
                }
                ClientFlowEvent::IdleAccepted { .. } => {
                    println!("IDLE accepted!");
                }
                ClientFlowEvent::IdleRejected { handle, status, .. } => {
                    let body = match status {
                        Status::Tagged(Tagged { body, .. }) => body,
                        _ => unreachable!(),
                    };

                    // This `unwrap` can't fail because `active_tasks` contains all in-progress `Commands`.
                    let (_, _, task) = self.active_tasks.remove_by_handle(handle).unwrap();

                    let output = Some(task.process_tagged(body));

                    return Ok(SchedulerEvent::TaskFinished(TaskToken { handle, output }));
                }
                ClientFlowEvent::IdleDoneSent { .. } => {
                    println!("IDLE done!");
                }
            }
        }
    }

    // ======== capabilities

    pub fn supported_capabilities(&self) -> slice::Iter<Capability> {
        self.capabilities.as_ref().iter()
    }

    pub fn supported_auth_mechanisms(&self) -> HashSet<AuthMechanism<'static>> {
        self.supported_capabilities()
            .filter_map(|capability| {
                if let Capability::Auth(mechanism) = capability {
                    Some(mechanism.to_static())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn supported_threading_algorithms(&self) -> HashSet<ThreadingAlgorithm<'static>> {
        self.supported_capabilities()
            .filter_map(|capability| {
                if let Capability::Thread(algorithm) = capability {
                    Some(algorithm.to_static())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn ext_sasl_ir_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::SaslIr))
    }

    pub fn ext_id_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::Id))
    }

    pub fn ext_uidplus_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::UidPlus))
    }

    pub fn ext_sort_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::Sort(_)))
    }

    pub fn ext_thread_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::Thread(_)))
    }

    pub fn ext_idle_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::Idle))
    }

    pub fn ext_binary_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::Binary))
    }

    pub fn ext_move_supported(&self) -> bool {
        self.supported_capabilities()
            .any(|c| matches!(c, Capability::Move))
    }

    // ======== tasks

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn capability(&mut self) -> Result<CapabilityTaskOutput, SchedulerError> {
        self.exec_task(CapabilityTask::new()).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn id<'a>(
        &mut self,
        params: Vec<(IString<'a>, NString<'a>)>,
    ) -> Result<IdTaskOutput, SchedulerError> {
        self.exec_task(IdTask::new(params.into_static())).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn create<'a>(
        &mut self,
        mbox: Mailbox<'a>,
    ) -> Result<CreateTaskOutput, SchedulerError> {
        self.exec_task(CreateTask::new(mbox.into_static())).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn list<'a>(
        &mut self,
        mbox: Mailbox<'a>,
        mbox_wildcard: ListMailbox<'a>,
    ) -> Result<ListTaskOutput, SchedulerError> {
        self.exec_task(ListTask::new(
            mbox.into_static(),
            mbox_wildcard.into_static(),
        ))
        .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn select<'a>(
        &mut self,
        mbox: Mailbox<'a>,
        read_only: bool,
    ) -> Result<SelectTaskOutput, SchedulerError> {
        self.exec_task(SelectTask::new(mbox.into_static(), read_only))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn expunge(&mut self) -> Result<ExpungeTaskOutput, SchedulerError> {
        self.exec_task(ExpungeTask::new()).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn delete<'a>(
        &mut self,
        mbox: Mailbox<'a>,
    ) -> Result<DeleteTaskOutput, SchedulerError> {
        self.exec_task(DeleteTask::new(mbox.into_static())).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn fetch<'a>(
        &mut self,
        seq: SequenceSet,
        items: MacroOrMessageDataItemNames<'a>,
        uid: bool,
    ) -> Result<FetchTaskOutput, SchedulerError> {
        self.exec_task(FetchTask::new(seq, items.into_static(), uid))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn fetch_first<'a>(
        &mut self,
        seq: SequenceSet,
        items: MacroOrMessageDataItemNames<'a>,
        uid: bool,
    ) -> Result<FetchFirstTaskOutput, SchedulerError> {
        self.exec_task(FetchFirstTask::new(seq, items.into_static(), uid))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn search<'a>(
        &mut self,
        criteria: Vec1<SearchKey<'a>>,
        uid: bool,
    ) -> Result<SearchTaskOutput, SchedulerError> {
        self.exec_task(SearchTask::new(criteria.into_static(), uid))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn sort<'a>(
        &mut self,
        sort_criteria: Vec1<SortCriterion>,
        charset: Charset<'a>,
        search_criteria: Vec1<SearchKey<'a>>,
        uid: bool,
    ) -> Result<SortTaskOutput, SchedulerError> {
        self.exec_task(SortTask::new(
            sort_criteria,
            charset.into_static(),
            search_criteria.into_static(),
            uid,
        ))
        .await?
    }
    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn sort_or_search_then_fetch<'a>(
        &mut self,
        sort_criteria: Vec1<SortCriterion>,
        charset: Charset<'a>,
        search_criteria: Vec1<SearchKey<'a>>,
        mut fetch_items: Vec<MessageDataItemName<'a>>,
        uid: bool,
    ) -> Result<Vec<Vec1<MessageDataItem<'a>>>, SchedulerError> {
        if self.ext_sort_supported() {
            let uids = self
                .exec_task(SortTask::new(
                    sort_criteria,
                    charset.into_static(),
                    search_criteria.into_static(),
                    uid,
                ))
                .await??;

            let fetches = self
                .fetch(uids.try_into().unwrap(), fetch_items.into(), uid)
                .await?;

            Ok(fetches.into_values().collect())
        } else {
            debug!("IMAP SORT extension not available, using fallback");
            let uids = self.search(search_criteria, uid).await?;

            sort_criteria
                .as_ref()
                .iter()
                .filter_map(|criterion| match criterion.key {
                    SortKey::Arrival => Some(MessageDataItemName::InternalDate),
                    SortKey::Cc => Some(MessageDataItemName::Envelope),
                    SortKey::Date => Some(MessageDataItemName::Envelope),
                    SortKey::From => Some(MessageDataItemName::Envelope),
                    SortKey::Size => Some(MessageDataItemName::Rfc822Size),
                    SortKey::Subject => Some(MessageDataItemName::Envelope),
                    SortKey::To => Some(MessageDataItemName::Envelope),
                    SortKey::DisplayFrom => None,
                    SortKey::DisplayTo => None,
                })
                .for_each(|item| {
                    if !fetch_items.contains(&item) {
                        fetch_items.push(item)
                    }
                });

            let mut fetches: Vec<_> = self
                .fetch(uids.try_into().unwrap(), fetch_items.into(), uid)
                .await?
                .into_values()
                .collect();

            fetches.sort_by(|a, b| {
                for criterion in sort_criteria.as_ref() {
                    let mut cmp = cmp_fetch_items(&criterion.key, a, b);

                    if criterion.reverse {
                        cmp = cmp.reverse();
                    }

                    if cmp.is_ne() {
                        return cmp;
                    }
                }

                cmp_fetch_items(&SortKey::Date, a, b)
            });

            Ok(fetches)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn thread<'a>(
        &mut self,
        algorithm: ThreadingAlgorithm<'a>,
        charset: Charset<'a>,
        search_criteria: Vec1<SearchKey<'a>>,
        uid: bool,
    ) -> Result<ThreadTaskOutput, SchedulerError> {
        self.exec_task(ThreadTask::new(
            algorithm.into_static(),
            charset.into_static(),
            search_criteria.into_static(),
            uid,
        ))
        .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn idle(&mut self, done: mpsc::Sender<()>) -> Result<IdleTaskOutput, SchedulerError> {
        if self.ext_idle_supported() {
            self.exec_task(IdleTask::new(done)).await?
        } else {
            debug!("IMAP IDLE extension not available, falling back to poll");
            tokio::time::sleep(self.get_idle_timeout()).await;
            done.send(()).await.map_err(ClientFlowError::QueueClosed)?;
            Ok(())
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn append<'a>(
        &mut self,
        mbox: Mailbox<'a>,
        flags: Vec<Flag<'a>>,
        msg: Cow<'a, [u8]>,
    ) -> Result<AppendTaskOutput, SchedulerError> {
        let msg = if self.ext_binary_supported() {
            LiteralOrLiteral8::Literal8(Literal8 {
                data: msg,
                mode: LiteralMode::Sync,
            })
        } else {
            debug!("IMAP BINARY extension not available, using fallback");
            Literal::validate(msg.as_ref())?;
            LiteralOrLiteral8::Literal(Literal::unvalidated(msg))
        };

        self.exec_task(AppendTask::new(
            mbox.into_static(),
            flags.into_static(),
            msg.into_static(),
        ))
        .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn appenduid<'a>(
        &mut self,
        mbox: Mailbox<'a>,
        flags: Vec<Flag<'a>>,
        msg: Cow<'a, [u8]>,
    ) -> Result<AppendUidTaskOutput, SchedulerError> {
        if self.ext_uidplus_supported() {
            let msg = if self.ext_binary_supported() {
                LiteralOrLiteral8::Literal8(Literal8 {
                    data: msg,
                    mode: LiteralMode::Sync,
                })
            } else {
                debug!("IMAP BINARY extension not available, using fallback");
                Literal::validate(msg.as_ref())?;
                LiteralOrLiteral8::Literal(Literal::unvalidated(msg))
            };

            self.exec_task(AppendUidTask::new(
                mbox.into_static(),
                flags.into_static(),
                msg.into_static(),
            ))
            .await?
        } else {
            debug!("IMAP UIDPLUS extension not available, using fallback");
            // If the mailbox is currently selected, the normal new
            // message actions SHOULD occur.  Specifically, the server
            // SHOULD notify the client immediately via an untagged
            // EXISTS response.  If the server does not do so, the
            // client MAY issue a NOOP command (or failing that, a
            // CHECK command) after one or more APPEND commands.
            //
            // <https://datatracker.ietf.org/doc/html/rfc3501#section-6.3.11>
            self.select(mbox.clone(), false).await?;

            let seq = match self.append(mbox, flags, msg).await? {
                Some(seq) => seq,
                None => match self.post_append_noop().await? {
                    Some(seq) => seq,
                    None => self
                        .post_append_check()
                        .await?
                        .ok_or(SchedulerError::MissingData("APPEND: seq".into()))?,
                },
            };

            let uid = self
                .search(
                    Vec1::from(SearchKey::SequenceSet(seq.try_into().unwrap())),
                    true,
                )
                .await?
                .into_iter()
                .next();

            Ok(uid)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn post_append_noop(&mut self) -> Result<PostAppendNoOpTaskOutput, SchedulerError> {
        self.exec_task(PostAppendNoOpTask::new()).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn post_append_check(&mut self) -> Result<PostAppendCheckTaskOutput, SchedulerError> {
        self.exec_task(PostAppendCheckTask::new()).await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn copy<'a>(
        &mut self,
        seq: SequenceSet,
        mbox: Mailbox<'a>,
        uid: bool,
    ) -> Result<CopyTaskOutput, SchedulerError> {
        self.exec_task(CopyTask::new(seq, mbox.into_static(), uid))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn r#move<'a>(
        &mut self,
        seq: SequenceSet,
        mbox: Mailbox<'a>,
        uid: bool,
    ) -> Result<MoveTaskOutput, SchedulerError> {
        if self.ext_move_supported() {
            self.exec_task(MoveTask::new(seq, mbox.into_static(), uid))
                .await?
        } else {
            debug!("IMAP MOVE extension not available, falling back to copy/store/expunge");
            self.copy(seq.clone(), mbox, uid).await?;
            self.silent_store(seq, StoreType::Add, vec![Flag::Deleted], uid)
                .await?;
            self.expunge().await?;
            Ok(())
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn store<'a>(
        &mut self,
        seq: SequenceSet,
        kind: StoreType,
        flags: Vec<Flag<'a>>,
        uid: bool,
    ) -> Result<StoreTaskOutput, SchedulerError> {
        self.exec_task(StoreTask::new(seq, kind, flags.into_static(), uid))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn silent_store<'a>(
        &mut self,
        seq: SequenceSet,
        kind: StoreType,
        flags: Vec<Flag<'a>>,
        uid: bool,
    ) -> Result<SilentStoreTaskOutput, SchedulerError> {
        self.exec_task(SilentStoreTask::new(seq, kind, flags.into_static(), uid))
            .await?
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    pub async fn noop(&mut self) -> Result<NoOpTaskOutput, SchedulerError> {
        self.exec_task(NoOpTask::new()).await?
    }
}

fn cmp_fetch_items(
    criterion: &SortKey,
    a: &Vec1<MessageDataItem>,
    b: &Vec1<MessageDataItem>,
) -> Ordering {
    use MessageDataItem::*;

    match &criterion {
        SortKey::Arrival => {
            let a = a.as_ref().iter().find_map(|a| {
                if let InternalDate(dt) = a {
                    Some(dt.as_ref())
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let InternalDate(dt) = b {
                    Some(dt.as_ref())
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Date => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Envelope(envelope) = a {
                    envelope.date.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Envelope(envelope) = b {
                    envelope.date.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Size => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Rfc822Size(size) = a {
                    Some(size)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Rfc822Size(size) = b {
                    Some(size)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        SortKey::Subject => {
            let a = a.as_ref().iter().find_map(|a| {
                if let Envelope(envelope) = a {
                    envelope.subject.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            let b = b.as_ref().iter().find_map(|b| {
                if let Envelope(envelope) = b {
                    envelope.subject.0.as_ref().map(AsRef::as_ref)
                } else {
                    None
                }
            });

            a.cmp(&b)
        }
        // FIXME: Address missing Ord derive in imap-types
        SortKey::Cc | SortKey::From | SortKey::To | SortKey::DisplayFrom | SortKey::DisplayTo => {
            Ordering::Equal
        }
    }
}

#[derive(Debug, Default)]
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

    #[error(transparent)]
    Validation(#[from] ValidationError),

    #[error("unexpected NO response: {}", .0.text)]
    No(StatusBody<'static>),

    #[error("unexpected BAD response: {}", .0.text)]
    Bad(StatusBody<'static>),

    #[error("missing data for command {0}")]
    MissingData(String),

    /// Unexpected tag in command completion result.
    ///
    /// The scheduler received a tag that cannot be matched to an active command.
    /// This could be due to a severe implementation error in the scheduler,
    /// the server, or anything in-between, really.
    ///
    /// It's better to halt the execution to avoid damage.
    #[error("unexpected tag in command completion result")]
    UnexpectedTaggedResponse(Tagged<'static>),

    #[error("unexpected unsolicited response")]
    UnexpectedByeResponse(Bye<'static>),
}

#[derive(Eq)]
pub struct TaskHandle<T: Task> {
    handle: ClientFlowCommandHandle,
    _t: PhantomData<T>,
}

impl<T: Task> fmt::Debug for TaskHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
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
trait TaskAny: fmt::Debug + Send {
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
    ) -> Result<AuthenticateData<'static>, CommandContinuationRequest<'static>>;

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
    ) -> Result<AuthenticateData<'static>, CommandContinuationRequest<'static>> {
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
