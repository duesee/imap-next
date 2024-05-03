use imap_types::{
    command::CommandBody,
    extensions::binary::LiteralOrLiteral8,
    flag::Flag,
    mailbox::Mailbox,
    response::{Data, StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type AppendTaskOutput = Option<u32>;

#[derive(Clone, Debug)]
pub struct AppendTask {
    mailbox: Mailbox<'static>,
    flags: Vec<Flag<'static>>,
    message: LiteralOrLiteral8<'static>,
    output: AppendTaskOutput,
}

impl AppendTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(
        mailbox: Mailbox<'static>,
        flags: Vec<Flag<'static>>,
        message: LiteralOrLiteral8<'static>,
    ) -> Self {
        Self {
            mailbox,
            flags,
            message,
            output: None,
        }
    }
}

impl Task for AppendTask {
    type Output = Result<AppendTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Append {
            mailbox: self.mailbox.clone(),
            flags: self.flags.clone(),
            date: None,
            message: self.message.clone(),
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip_all))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Exists(seq) = data {
            self.output = Some(seq);
            None
        } else {
            Some(data)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}

pub type PostAppendNoOpTaskOutput = Option<u32>;

#[derive(Clone, Debug)]
pub struct PostAppendNoOpTask {
    output: PostAppendNoOpTaskOutput,
}

impl PostAppendNoOpTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new() -> Self {
        Self { output: None }
    }
}

impl Task for PostAppendNoOpTask {
    type Output = Result<PostAppendNoOpTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Noop
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Exists(seq) = data {
            self.output = Some(seq);
            None
        } else {
            Some(data)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}

pub type PostAppendCheckTaskOutput = Option<u32>;

#[derive(Clone, Debug)]
pub struct PostAppendCheckTask {
    output: PostAppendCheckTaskOutput,
}

impl PostAppendCheckTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new() -> Self {
        Self { output: None }
    }
}

impl Task for PostAppendCheckTask {
    type Output = Result<PostAppendCheckTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Check
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Exists(seq) = data {
            self.output = Some(seq);
            None
        } else {
            Some(data)
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(SchedulerError::No(status_body)),
            StatusKind::Bad => Err(SchedulerError::Bad(status_body)),
        }
    }
}
