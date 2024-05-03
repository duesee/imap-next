use imap_types::{
    command::CommandBody,
    core::QuotedChar,
    flag::FlagNameAttribute,
    mailbox::{ListMailbox, Mailbox},
    response::{Data, StatusBody, StatusKind},
};

use crate::{SchedulerError, Task};

pub type ListTaskOutput = Vec<(
    Mailbox<'static>,
    Option<QuotedChar>,
    Vec<FlagNameAttribute<'static>>,
)>;

#[derive(Clone, Debug)]
pub struct ListTask {
    mailbox: Mailbox<'static>,
    mailbox_wildcard: ListMailbox<'static>,
    output: ListTaskOutput,
}

impl ListTask {
    #[cfg_attr(debug_assertions, tracing::instrument)]
    pub fn new(mailbox: Mailbox<'static>, mailbox_wildcard: ListMailbox<'static>) -> Self {
        Self {
            mailbox,
            mailbox_wildcard,
            output: Default::default(),
        }
    }
}

impl Task for ListTask {
    type Output = Result<ListTaskOutput, SchedulerError>;

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::List {
            reference: self.mailbox.clone(),
            mailbox_wildcard: self.mailbox_wildcard.clone(),
        }
    }

    #[cfg_attr(debug_assertions, tracing::instrument(skip(self)))]
    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::List {
            items,
            delimiter,
            mailbox,
        } = data
        {
            self.output.push((mailbox, delimiter, items));
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
