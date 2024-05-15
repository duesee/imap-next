use imap_types::{
    command::CommandBody,
    datetime::DateTime,
    extensions::binary::LiteralOrLiteral8,
    flag::Flag,
    mailbox::Mailbox,
    response::{Data, StatusBody, StatusKind},
};
use tracing::warn;

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct AppendTask {
    mailbox: Mailbox<'static>,
    flags: Vec<Flag<'static>>,
    date: Option<DateTime>,
    message: LiteralOrLiteral8<'static>,
    output: Option<u32>,
}

impl Task for AppendTask {
    type Output = Result<Option<u32>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Append {
            mailbox: self.mailbox.clone(),
            flags: self.flags.clone(),
            date: self.date.clone(),
            message: self.message.clone(),
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        // In case the mailbox is already selected, we should receive
        // an `EXISTS` response.
        if let Data::Exists(seq) = data {
            if self.output.is_some() {
                warn!("received duplicate APPEND EXISTS data");
            }
            self.output = Some(seq);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl AppendTask {
    pub fn new(mailbox: Mailbox<'static>, message: LiteralOrLiteral8<'static>) -> Self {
        Self {
            mailbox,
            flags: Default::default(),
            date: Default::default(),
            message,
            output: Default::default(),
        }
    }

    pub fn set_flags(&mut self, flags: Vec<Flag<'static>>) {
        self.flags = flags;
    }

    pub fn add_flag(&mut self, flag: Flag<'static>) {
        self.flags.push(flag);
    }

    pub fn with_flags(mut self, flags: Vec<Flag<'static>>) -> Self {
        self.set_flags(flags);
        self
    }

    pub fn with_flag(mut self, flag: Flag<'static>) -> Self {
        self.add_flag(flag);
        self
    }

    pub fn set_date(&mut self, date: DateTime) {
        self.date = Some(date);
    }

    pub fn with_date(mut self, date: DateTime) -> Self {
        self.set_date(date);
        self
    }
}

/// Special [`NoOpTask`](super::noop::NoOpTask) that captures `EXISTS`
/// responses.
///
/// This task should be used whenever [`AppendTask`] does not return
/// the number of messages in the mailbox the appended message
/// resides.
#[derive(Clone, Debug, Default)]
pub struct PostAppendNoOpTask {
    output: Option<u32>,
}

impl Task for PostAppendNoOpTask {
    type Output = Result<Option<u32>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Noop
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Exists(seq) = data {
            self.output = Some(seq);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl PostAppendNoOpTask {
    pub fn new() -> Self {
        Default::default()
    }
}

/// Special [`CheckTask`](super::check::CheckTask) that captures
/// `EXISTS` responses.
///
/// This task should be used whenever [`AppendTask`] and
/// [`PostAppendNoOpTask`] do not return the number of messages in the
/// mailbox the appended message resides.
#[derive(Clone, Debug, Default)]
pub struct PostAppendCheckTask {
    output: Option<u32>,
}

impl Task for PostAppendCheckTask {
    type Output = Result<Option<u32>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Check
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Exists(seq) = data {
            self.output = Some(seq);
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl PostAppendCheckTask {
    pub fn new() -> Self {
        Default::default()
    }
}
