use std::{cmp::Ordering, collections::HashMap, num::NonZeroU32, sync::Arc, time::Duration};

pub use imap_flow;
use imap_flow::{
    client::{ClientFlow, ClientFlowError, ClientFlowEvent, ClientFlowOptions},
    imap_codec::imap_types::{
        auth::AuthMechanism,
        bounded_static::IntoBoundedStatic,
        command::{Command, CommandBody},
        core::{IString, Literal, LiteralMode, NString, QuotedChar, Tag, Vec1},
        error::ValidationError,
        extensions::{
            binary::{Literal8, LiteralOrLiteral8},
            sort::{SortCriterion, SortKey},
            thread::{Thread, ThreadingAlgorithm},
        },
        fetch::{MacroOrMessageDataItemNames, MessageDataItem, MessageDataItemName},
        flag::{Flag, FlagNameAttribute, StoreType},
        mailbox::{ListMailbox, Mailbox},
        response::{Capability, Code, Status, Tagged},
        search::SearchKey,
        sequence::SequenceSet,
    },
    stream::{Stream, StreamError},
};
use once_cell::sync::Lazy;
pub use tasks;
use tasks::{
    resolver::Resolver,
    tasks::{
        append::{AppendTask, PostAppendCheckTask, PostAppendNoOpTask},
        appenduid::AppendUidTask,
        authenticate::AuthenticateTask,
        capability::CapabilityTask,
        check::CheckTask,
        copy::CopyTask,
        create::CreateTask,
        delete::DeleteTask,
        expunge::ExpungeTask,
        fetch::{FetchFirstTask, FetchTask},
        id::IdTask,
        list::ListTask,
        noop::NoOpTask,
        r#move::MoveTask,
        search::SearchTask,
        select::{SelectDataUnvalidated, SelectTask},
        sort::SortTask,
        starttls::StartTlsTask,
        store::StoreTask,
        thread::ThreadTask,
        TaskError,
    },
    SchedulerError, SchedulerEvent, Task,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConfig, RootCertStore},
    TlsConnector, TlsStream,
};
use tracing::{debug, trace, warn};

static ROOT_CERT_STORE: Lazy<RootCertStore> = Lazy::new(|| {
    let mut root_store = RootCertStore::empty();

    for cert in rustls_native_certs::load_native_certs().unwrap() {
        root_store.add(cert).unwrap();
    }

    root_store
});

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("cannot connect to server using TCP")]
    ConnectTcp(#[source] tokio::io::Error),
    #[error("cannot connect to server using TLS")]
    ConnectTls(#[source] tokio::io::Error),

    #[error("stream error")]
    Stream(#[from] StreamError<SchedulerError>),
    #[error("validation error")]
    Validation(#[from] ValidationError),

    #[error("cannot receive greeting from server")]
    ReceiveGreeting(#[source] StreamError<SchedulerError>),

    #[error("cannot resolve IMAP task")]
    ResolveTask(#[from] TaskError),
}

pub struct Client {
    host: String,
    stream: Stream,
    resolver: Resolver,
    capabilities: Vec1<Capability<'static>>,
    idle_timeout: Duration,
}

/// Client constructors.
///
/// This section defines 3 public constructors for [`Client`]:
/// `insecure`, `tls` and `starttls`.
impl Client {
    /// Creates an insecure client, using TCP.
    ///
    /// This constructor creates a client based on an raw
    /// [`TcpStream`], receives greeting then saves server
    /// capabilities.
    pub async fn insecure(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let mut client = Self::tcp(host, port).await?;

        if !client.receive_greeting().await? {
            client.refresh_capabilities().await?;
        }

        Ok(client)
    }

    /// Creates a secure client, using SSL/TLS.
    ///
    /// This constructor creates an client based on a secure
    /// [`TcpStream`] wrapped into a [`TlsStream`], receives greeting
    /// then saves server capabilities.
    pub async fn tls(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let tcp = Self::tcp(host, port).await?;
        Self::upgrade_tls(tcp, false).await
    }

    /// Creates a secure client, using STARTTLS.
    ///
    /// This constructor creates an insecure client based on a raw
    /// [`TcpStream`], receives greeting, wraps the [`TcpStream`] into
    /// a secured [`TlsStream`] then saves server capabilities.
    pub async fn starttls(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let tcp = Self::tcp(host, port).await?;
        Self::upgrade_tls(tcp, true).await
    }

    /// Creates an insecure client based on a raw [`TcpStream`].
    ///
    /// This function is internally used by public constructors
    /// `insecure`, `tls` and `starttls`.
    async fn tcp(host: impl ToString, port: u16) -> Result<Self, ClientError> {
        let host = host.to_string();

        let tcp_stream = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(ClientError::ConnectTcp)?;

        let stream = Stream::insecure(tcp_stream);

        let mut flow_opts = ClientFlowOptions::default();
        flow_opts.crlf_relaxed = true;

        let flow = ClientFlow::new(flow_opts);
        let resolver = Resolver::new(flow);

        Ok(Self {
            host,
            stream,
            resolver,
            capabilities: Vec1::from(Capability::Imap4Rev1),
            idle_timeout: Duration::from_secs(5 * 60), // 5 min
        })
    }

    /// Turns an insecure client into a secure one.
    ///
    /// The flow changes depending on the `starttls` parameter:
    ///
    /// If `true`: receives greeting, sends STARTTLS command, upgrades
    /// to TLS then force-refreshes server capabilities.
    ///
    /// If `false`: upgrades straight to TLS, receives greeting then
    /// refreshes server capabilities if needed.
    async fn upgrade_tls(mut self, starttls: bool) -> Result<Self, ClientError> {
        if starttls {
            self.receive_greeting().await?;
            let _ = self
                .stream
                .progress(self.resolver.resolve(StartTlsTask::new()))
                .await;
        }

        let mut config = ClientConfig::builder()
            .with_root_certificates(ROOT_CERT_STORE.clone())
            .with_no_client_auth();

        // See <https://www.iana.org/assignments/tls-extensiontype-values/tls-extensiontype-values.xhtml#alpn-protocol-ids>
        config.alpn_protocols = vec![b"imap".to_vec()];

        let connector = TlsConnector::from(Arc::new(config));
        let dnsname = ServerName::try_from(self.host.clone()).unwrap();

        let tls_stream = connector
            .connect(dnsname, self.stream.into_inner())
            .await
            .map_err(ClientError::ConnectTls)?;

        self.stream = Stream::tls(TlsStream::Client(tls_stream));

        if starttls || !self.receive_greeting().await? {
            self.refresh_capabilities().await?;
        }

        Ok(self)
    }

    /// Receives server greeting.
    ///
    /// Returns `true` if server capabilities were found in the
    /// greeting, otherwise `false`. This boolean is internally used
    /// to determine if server capabilities need to be explicitly
    /// requested or not.
    async fn receive_greeting(&mut self) -> Result<bool, ClientError> {
        let evt = self
            .stream
            .progress(&mut self.resolver)
            .await
            .map_err(ClientError::ReceiveGreeting)?;

        if let SchedulerEvent::GreetingReceived(greeting) = evt {
            if let Some(Code::Capability(capabilities)) = greeting.code {
                self.capabilities = capabilities;
                return Ok(true);
            }
        }

        Ok(false)
    }
}

/// Client getters and setters.
///
/// This section defines helpers to easily manipulate the client's
/// parameters and data.
impl Client {
    pub fn get_idle_timeout(&self) -> &Duration {
        &self.idle_timeout
    }

    pub fn set_idle_timeout(&mut self, timeout: Duration) {
        self.idle_timeout = timeout;
    }

    pub fn set_some_idle_timeout(&mut self, timeout: Option<Duration>) {
        if let Some(timeout) = timeout {
            self.set_idle_timeout(timeout)
        }
    }

    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.set_idle_timeout(timeout);
        self
    }

    pub fn with_some_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.set_some_idle_timeout(timeout);
        self
    }

    /// Returns the server capabilities.
    ///
    /// This function does not *fetch* capabilities from server, it
    /// just returns capabilities saved during the creation of this
    /// client (using [`Client::insecure`], [`Client::tls`] or
    /// [`Client::starttls`]).
    pub fn capabilities(&self) -> &Vec1<Capability<'static>> {
        &self.capabilities
    }

    /// Returns the server capabilities, as an iterator.
    ///
    /// Same as [`Client::capabilities`], but just returns an iterator
    /// instead.
    pub fn capabilities_iter(&self) -> impl Iterator<Item = &Capability<'static>> + '_ {
        self.capabilities().as_ref().iter()
    }

    /// Returns supported authentication mechanisms, as an iterator.
    pub fn supported_auth_mechanisms(&self) -> impl Iterator<Item = &AuthMechanism<'static>> + '_ {
        self.capabilities_iter().filter_map(|capability| {
            if let Capability::Auth(mechanism) = capability {
                Some(mechanism)
            } else {
                None
            }
        })
    }

    /// Returns `true` if the given authentication mechanism is
    /// supported by the server.
    pub fn supports_auth_mechanism(&self, mechanism: AuthMechanism<'static>) -> bool {
        self.capabilities_iter().any(|capability| {
            if let Capability::Auth(m) = capability {
                m == &mechanism
            } else {
                false
            }
        })
    }

    /// Returns `true` if the `SASL-IR` extension is supported by the
    /// server.
    pub fn ext_sasl_ir_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::SaslIr))
    }

    /// Returns `true` if the `ID` extension is supported by the
    /// server.
    pub fn ext_id_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Id))
    }

    /// Returns `true` if the `UIDPLUS` extension is supported by the
    /// server.
    pub fn ext_uidplus_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::UidPlus))
    }

    /// Returns `true` if the `SORT` extension is supported by the
    /// server.
    pub fn ext_sort_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Sort(_)))
    }

    /// Returns `true` if the `THREAD` extension is supported by the
    /// server.
    pub fn ext_thread_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Thread(_)))
    }

    /// Returns `true` if the `IDLE` extension is supported by the
    /// server.
    pub fn ext_idle_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Idle))
    }

    /// Returns `true` if the `BINARY` extension is supported by the
    /// server.
    pub fn ext_binary_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Binary))
    }

    /// Returns `true` if the `MOVE` extension is supported by the
    /// server.
    pub fn ext_move_supported(&self) -> bool {
        self.capabilities_iter()
            .any(|c| matches!(c, Capability::Move))
    }
}

/// Client low-level API.
///
/// This section defines the low-level API of the client, by exposing
/// convenient wrappers around [`Task`]s. They do not contain any
/// logic.
impl Client {
    /// Resolves the given [`Task`].
    pub async fn resolve<T: Task>(&mut self, task: T) -> Result<T::Output, ClientError> {
        Ok(self.stream.progress(self.resolver.resolve(task)).await?)
    }

    /// Creates a new mailbox.
    pub async fn create(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(CreateTask::new(mbox)).await??)
    }

    /// Lists mailboxes.
    pub async fn list_mailboxes(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        mailbox_wildcard: impl TryInto<ListMailbox<'_>, Error = ValidationError>,
    ) -> Result<
        Vec<(
            Mailbox<'static>,
            Option<QuotedChar>,
            Vec<FlagNameAttribute<'static>>,
        )>,
        ClientError,
    > {
        let mbox = mailbox.try_into()?.into_static();
        let mbox_wcard = mailbox_wildcard.try_into()?.into_static();
        Ok(self.resolve(ListTask::new(mbox, mbox_wcard)).await??)
    }

    /// Selects the given mailbox.
    pub async fn select(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<SelectDataUnvalidated, ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(SelectTask::new(mbox)).await??)
    }

    /// Selects the given mailbox in read-only mode.
    pub async fn examine(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<SelectDataUnvalidated, ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(SelectTask::read_only(mbox)).await??)
    }

    /// Expunges the selected mailbox.
    ///
    /// A mailbox needs to be selected before, otherwise this function
    /// will fail.
    pub async fn expunge(&mut self) -> Result<Vec<NonZeroU32>, ClientError> {
        Ok(self.resolve(ExpungeTask::new()).await??)
    }

    /// Deletes the given mailbox.
    pub async fn delete(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();
        Ok(self.resolve(DeleteTask::new(mbox)).await??)
    }

    /// Searches messages matching the given criteria.
    async fn _search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        let criteria: Vec<_> = criteria
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let criteria = if criteria.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(criteria).unwrap()
        };

        Ok(self
            .resolve(SearchTask::new(criteria).with_uid(uid))
            .await??)
    }

    /// Searches messages matching the given criteria.
    ///
    /// This function returns sequence numbers, if you need UID see
    /// [`Client::uid_search`].
    pub async fn search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._search(criteria, false).await
    }

    /// Searches messages matching the given criteria.
    ///
    /// This function returns UIDs, if you need sequence numbers see
    /// [`Client::search`].
    pub async fn uid_search(
        &mut self,
        criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._search(criteria, true).await
    }

    /// Searches messages matching the given search criteria, sorted
    /// by the given sort criteria.
    async fn _sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        let sort: Vec<_> = sort_criteria.into_iter().collect();
        let sort = if sort.is_empty() {
            Vec1::from(SortCriterion {
                reverse: true,
                key: SortKey::Date,
            })
        } else {
            Vec1::try_from(sort).unwrap()
        };

        let search: Vec<_> = search_criteria
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();
        let search = if search.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(search).unwrap()
        };

        Ok(self
            .resolve(SortTask::new(sort, search).with_uid(uid))
            .await??)
    }

    /// Searches messages matching the given search criteria, sorted
    /// by the given sort criteria.
    ///
    /// This function returns sequence numbers, if you need UID see
    /// [`Client::uid_sort`].
    pub async fn sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._sort(sort_criteria, search_criteria, false).await
    }

    /// Searches messages matching the given search criteria, sorted
    /// by the given sort criteria.
    ///
    /// This function returns UIDs, if you need sequence numbers see
    /// [`Client::sort`].
    pub async fn uid_sort(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<NonZeroU32>, ClientError> {
        self._sort(sort_criteria, search_criteria, true).await
    }

    async fn _thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        uid: bool,
    ) -> Result<Vec<Thread>, ClientError> {
        let alg = algorithm.into_static();

        let search: Vec<_> = search_criteria
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();
        let search = if search.is_empty() {
            Vec1::from(SearchKey::All)
        } else {
            Vec1::try_from(search).unwrap()
        };

        Ok(self
            .resolve(ThreadTask::new(alg, search).with_uid(uid))
            .await??)
    }

    pub async fn thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<Thread>, ClientError> {
        self._thread(algorithm, search_criteria, false).await
    }

    pub async fn uid_thread(
        &mut self,
        algorithm: ThreadingAlgorithm<'_>,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
    ) -> Result<Vec<Thread>, ClientError> {
        self._thread(algorithm, search_criteria, true).await
    }

    async fn _store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
        uid: bool,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        Ok(self
            .resolve(StoreTask::new(sequence_set, kind, flags).with_uid(uid))
            .await??)
    }

    pub async fn store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._store(sequence_set, kind, flags, false).await
    }

    pub async fn uid_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._store(sequence_set, kind, flags, true).await
    }

    async fn _silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let task = StoreTask::new(sequence_set, kind, flags)
            .with_uid(uid)
            .silent();

        Ok(self.resolve(task).await??)
    }

    pub async fn silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<(), ClientError> {
        self._silent_store(sequence_set, kind, flags, false).await
    }

    pub async fn uid_silent_store(
        &mut self,
        sequence_set: SequenceSet,
        kind: StoreType,
        flags: impl IntoIterator<Item = Flag<'_>>,
    ) -> Result<(), ClientError> {
        self._silent_store(sequence_set, kind, flags, true).await
    }

    pub async fn post_append_noop(&mut self) -> Result<Option<u32>, ClientError> {
        Ok(self.resolve(PostAppendNoOpTask::new()).await??)
    }

    pub async fn post_append_check(&mut self) -> Result<Option<u32>, ClientError> {
        Ok(self.resolve(PostAppendCheckTask::new()).await??)
    }

    async fn _fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        let items = items.into_static();

        Ok(self
            .resolve(FetchFirstTask::new(id, items).with_uid(uid))
            .await??)
    }

    pub async fn fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        self._fetch_first(id, items, false).await
    }

    pub async fn uid_fetch_first(
        &mut self,
        id: NonZeroU32,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec1<MessageDataItem<'static>>, ClientError> {
        self._fetch_first(id, items, true).await
    }

    async fn _copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        Ok(self
            .resolve(CopyTask::new(sequence_set, mbox).with_uid(uid))
            .await??)
    }

    pub async fn copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._copy(sequence_set, mailbox, false).await
    }

    pub async fn uid_copy(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._copy(sequence_set, mailbox, true).await
    }

    async fn _move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        uid: bool,
    ) -> Result<(), ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        Ok(self
            .resolve(MoveTask::new(sequence_set, mbox).with_uid(uid))
            .await??)
    }

    pub async fn r#move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move(sequence_set, mailbox, false).await
    }

    pub async fn uid_move(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move(sequence_set, mailbox, true).await
    }

    /// Executes the `CHECK` command.
    pub async fn check(&mut self) -> Result<(), ClientError> {
        Ok(self.resolve(CheckTask::new()).await??)
    }

    /// Executes the `NOOP` command.
    pub async fn noop(&mut self) -> Result<(), ClientError> {
        Ok(self.resolve(NoOpTask::new()).await??)
    }
}

/// Client medium-level API.
///
/// This section defines the medium-level API of the client (based on
/// the low-level one), by exposing helpers that update client state
/// and use a small amount of logic (mostly conditional code depending
/// on available server capabilities).
impl Client {
    /// Fetches server capabilities, then saves them.
    pub async fn refresh_capabilities(&mut self) -> Result<(), ClientError> {
        self.capabilities = self.resolve(CapabilityTask::new()).await??;
        Ok(())
    }

    /// Authenticates the user using the given [`AuthenticateTask`].
    ///
    /// This function also refreshes capabilities (either from the
    /// task output or from explicit request).
    async fn authenticate(&mut self, task: AuthenticateTask) -> Result<(), ClientError> {
        match self.resolve(task).await?? {
            Some(capabilities) => {
                self.capabilities = capabilities;
            }
            None => {
                self.refresh_capabilities().await?;
            }
        };

        Ok(())
    }

    /// Authenticates the user using the `PLAIN` mechanism.
    pub async fn authenticate_plain(
        &mut self,
        login: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        self.authenticate(AuthenticateTask::plain(
            login.as_ref(),
            password.as_ref(),
            self.ext_sasl_ir_supported(),
        ))
        .await
    }

    /// Authenticates the user using the `XOAUTH2` mechanism.
    pub async fn authenticate_xoauth2(
        &mut self,
        login: impl AsRef<str>,
        token: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        self.authenticate(AuthenticateTask::xoauth2(
            login.as_ref(),
            token.as_ref(),
            self.ext_sasl_ir_supported(),
        ))
        .await
    }

    /// Authenticates the user using the `OAUTHBEARER` mechanism.
    pub async fn authenticate_oauthbearer(
        &mut self,
        user: impl AsRef<str>,
        host: impl AsRef<str>,
        port: u16,
        token: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        self.authenticate(AuthenticateTask::oauthbearer(
            user.as_ref(),
            host.as_ref(),
            port,
            token.as_ref(),
            self.ext_sasl_ir_supported(),
        ))
        .await
    }

    /// Exchanges client/server ids.
    ///
    /// If the server does not support the `ID` extension, this
    /// function has no effect.
    pub async fn id(
        &mut self,
        params: Option<Vec<(IString<'static>, NString<'static>)>>,
    ) -> Result<Option<Vec<(IString<'static>, NString<'static>)>>, ClientError> {
        Ok(if self.ext_id_supported() {
            self.resolve(IdTask::new(params)).await??
        } else {
            warn!("IMAP ID extension not supported, skipping");
            None
        })
    }

    pub async fn append(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<u32>, ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let msg = to_static_literal(message, self.ext_binary_supported())?;

        Ok(self
            .resolve(AppendTask::new(mbox, msg).with_flags(flags))
            .await??)
    }

    pub async fn appenduid(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<(NonZeroU32, NonZeroU32)>, ClientError> {
        let mbox = mailbox.try_into()?.into_static();

        let flags: Vec<_> = flags
            .into_iter()
            .map(IntoBoundedStatic::into_static)
            .collect();

        let msg = to_static_literal(message, self.ext_binary_supported())?;

        Ok(self
            .resolve(AppendUidTask::new(mbox, msg).with_flags(flags))
            .await??)
    }
}

/// Client high-level API.
///
/// This section defines the high-level API of the client (based on
/// the low and medium ones), by exposing opinionated helpers. They
/// contain more logic, and make use of fallbacks depending on
/// available server capabilities.
impl Client {
    async fn _fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        let mut items = match items {
            MacroOrMessageDataItemNames::Macro(m) => m.expand().into_static(),
            MacroOrMessageDataItemNames::MessageDataItemNames(items) => items.into_static(),
        };

        if uid {
            items.push(MessageDataItemName::Uid);
        }

        let seq_map = self
            .resolve(FetchTask::new(sequence_set, items.into()).with_uid(uid))
            .await??;

        if uid {
            let mut uid_map = HashMap::new();

            for (seq, items) in seq_map {
                let uid = items.as_ref().iter().find_map(|item| {
                    if let MessageDataItem::Uid(uid) = item {
                        Some(*uid)
                    } else {
                        None
                    }
                });

                match uid {
                    Some(uid) => {
                        uid_map.insert(uid, items);
                    }
                    None => {
                        warn!(?seq, "cannot get message uid, skipping it");
                    }
                }
            }

            Ok(uid_map)
        } else {
            Ok(seq_map)
        }
    }

    pub async fn fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._fetch(sequence_set, items, false).await
    }

    pub async fn uid_fetch(
        &mut self,
        sequence_set: SequenceSet,
        items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<HashMap<NonZeroU32, Vec1<MessageDataItem<'static>>>, ClientError> {
        self._fetch(sequence_set, items, true).await
    }

    async fn _sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
        uid: bool,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        let mut fetch_items = match fetch_items {
            MacroOrMessageDataItemNames::Macro(m) => m.expand().into_static(),
            MacroOrMessageDataItemNames::MessageDataItemNames(items) => items,
        };

        if uid {
            fetch_items.push(MessageDataItemName::Uid);
        }

        if self.ext_sort_supported() {
            let fetch_items = MacroOrMessageDataItemNames::MessageDataItemNames(fetch_items);

            let ids = self.sort(sort_criteria, search_criteria).await?;

            let mut fetches = self
                ._fetch(ids.clone().try_into().unwrap(), fetch_items, uid)
                .await?;

            let items = ids.into_iter().flat_map(|id| fetches.remove(&id)).collect();

            Ok(items)
        } else {
            warn!("IMAP SORT extension not supported, using fallback");
            let ids = self._search(search_criteria, uid).await?;

            sort_criteria
                .clone()
                .into_iter()
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
                ._fetch(ids.try_into().unwrap(), fetch_items.into(), uid)
                .await?
                .into_values()
                .collect();

            fetches.sort_by(|a, b| {
                for criterion in sort_criteria.clone().into_iter() {
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

    pub async fn sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        self._sort_or_fallback(sort_criteria, search_criteria, fetch_items, false)
            .await
    }

    pub async fn uid_sort_or_fallback(
        &mut self,
        sort_criteria: impl IntoIterator<Item = SortCriterion> + Clone,
        search_criteria: impl IntoIterator<Item = SearchKey<'_>>,
        fetch_items: MacroOrMessageDataItemNames<'_>,
    ) -> Result<Vec<Vec1<MessageDataItem<'static>>>, ClientError> {
        self._sort_or_fallback(sort_criteria, search_criteria, fetch_items, true)
            .await
    }

    pub async fn appenduid_or_fallback(
        &mut self,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError> + Clone,
        flags: impl IntoIterator<Item = Flag<'_>>,
        message: impl AsRef<[u8]>,
    ) -> Result<Option<NonZeroU32>, ClientError> {
        if self.ext_uidplus_supported() {
            Ok(self
                .appenduid(mailbox, flags, message)
                .await?
                .map(|(uid, _)| uid))
        } else {
            warn!("IMAP UIDPLUS extension not supported, using fallback");

            // If the mailbox is currently selected, the normal new
            // message actions SHOULD occur.  Specifically, the server
            // SHOULD notify the client immediately via an untagged
            // EXISTS response.  If the server does not do so, the
            // client MAY issue a NOOP command (or failing that, a
            // CHECK command) after one or more APPEND commands.
            //
            // <https://datatracker.ietf.org/doc/html/rfc3501#section-6.3.11>
            self.select(mailbox.clone()).await?;

            let seq = match self.append(mailbox, flags, message).await? {
                Some(seq) => seq,
                None => match self.post_append_noop().await? {
                    Some(seq) => seq,
                    None => self
                        .post_append_check()
                        .await?
                        .ok_or(ClientError::ResolveTask(TaskError::MissingData(
                            "APPENDUID: seq".into(),
                        )))?,
                },
            };

            let uid = self
                .search(Vec1::from(SearchKey::SequenceSet(seq.try_into().unwrap())))
                .await?
                .into_iter()
                .next();

            Ok(uid)
        }
    }

    async fn _move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
        uid: bool,
    ) -> Result<(), ClientError> {
        if self.ext_move_supported() {
            self._move(sequence_set, mailbox, uid).await
        } else {
            warn!("IMAP MOVE extension not supported, using fallback");
            self._copy(sequence_set.clone(), mailbox, uid).await?;
            self._silent_store(sequence_set, StoreType::Add, Some(Flag::Deleted), uid)
                .await?;
            self.expunge().await?;
            Ok(())
        }
    }

    pub async fn move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move_or_fallback(sequence_set, mailbox, false).await
    }

    pub async fn uid_move_or_fallback(
        &mut self,
        sequence_set: SequenceSet,
        mailbox: impl TryInto<Mailbox<'_>, Error = ValidationError>,
    ) -> Result<(), ClientError> {
        self._move_or_fallback(sequence_set, mailbox, true).await
    }

    pub fn enqueue_idle(&mut self) -> Tag<'static> {
        let tag = self.resolver.scheduler.tag_generator.generate();

        self.resolver.scheduler.flow.enqueue_command(Command {
            tag: tag.clone(),
            body: CommandBody::Idle,
        });

        tag.into_static()
    }

    #[tracing::instrument(name = "idle", skip_all)]
    pub async fn idle(&mut self, tag: Tag<'static>) -> Result<(), StreamError<ClientFlowError>> {
        debug!("starting the main loop");

        loop {
            let progress = self.stream.progress(&mut self.resolver.scheduler.flow);
            match tokio::time::timeout(self.idle_timeout, progress).await {
                Err(_) => {
                    debug!("timed out, sending done command…");
                    self.resolver.scheduler.flow.set_idle_done();
                }
                Ok(Err(err)) => {
                    break Err(err);
                }
                Ok(Ok(ClientFlowEvent::IdleCommandSent { .. })) => {
                    debug!("command sent");
                }
                Ok(Ok(ClientFlowEvent::IdleAccepted { .. })) => {
                    debug!("command accepted, entering idle mode");
                }
                Ok(Ok(ClientFlowEvent::IdleRejected { status, .. })) => {
                    warn!("command rejected, aborting: {status:?}");
                    break Ok(());
                }
                Ok(Ok(ClientFlowEvent::IdleDoneSent { .. })) => {
                    debug!("done command sent");
                }
                Ok(Ok(ClientFlowEvent::DataReceived { data })) => {
                    debug!("received data, sending done command…");
                    trace!("{data:#?}");
                    self.resolver.scheduler.flow.set_idle_done();
                }
                Ok(Ok(ClientFlowEvent::StatusReceived {
                    status:
                        Status::Tagged(Tagged {
                            tag: ref got_tag, ..
                        }),
                })) if *got_tag == tag => {
                    debug!("received tagged response, exiting");
                    break Ok(());
                }
                Ok(event) => {
                    debug!("received unknown event, ignoring: {event:?}");
                }
            }
        }
    }

    #[tracing::instrument(name = "idle/done", skip_all)]
    pub async fn idle_done(
        &mut self,
        tag: Tag<'static>,
    ) -> Result<(), StreamError<ClientFlowError>> {
        self.resolver.scheduler.flow.set_idle_done();

        loop {
            let progress = self
                .stream
                .progress(&mut self.resolver.scheduler.flow)
                .await?;

            match progress {
                ClientFlowEvent::IdleDoneSent { .. } => {
                    debug!("done command sent");
                }
                ClientFlowEvent::StatusReceived {
                    status:
                        Status::Tagged(Tagged {
                            tag: ref got_tag, ..
                        }),
                } if *got_tag == tag => {
                    debug!("received tagged response, exiting");
                    break Ok(());
                }
                event => {
                    debug!("received unknown event, ignoring: {event:?}");
                }
            }
        }
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

fn to_static_literal(
    message: impl AsRef<[u8]>,
    ext_binary_supported: bool,
) -> Result<LiteralOrLiteral8<'static>, ValidationError> {
    let message = if ext_binary_supported {
        LiteralOrLiteral8::Literal8(Literal8 {
            data: message.as_ref().into(),
            mode: LiteralMode::Sync,
        })
    } else {
        warn!("IMAP BINARY extension not supported, using fallback");
        Literal::validate(message.as_ref())?;
        LiteralOrLiteral8::Literal(Literal::unvalidated(message.as_ref()))
    };

    Ok(message.into_static())
}
