use imap_types::{
    command::CommandBody,
    core::QuotedChar,
    flag::FlagNameAttribute,
    mailbox::{ListMailbox, Mailbox},
    response::{Data, StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct ListTask {
    mailbox: Mailbox<'static>,
    mailbox_wildcard: ListMailbox<'static>,
    output: Vec<(
        Mailbox<'static>,
        Option<QuotedChar>,
        Vec<FlagNameAttribute<'static>>,
    )>,
}

impl Task for ListTask {
    type Output = Result<
        Vec<(
            Mailbox<'static>,
            Option<QuotedChar>,
            Vec<FlagNameAttribute<'static>>,
        )>,
        TaskError,
    >;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::List {
            reference: self.mailbox.clone(),
            mailbox_wildcard: self.mailbox_wildcard.clone(),
        }
    }

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

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.output),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl ListTask {
    pub fn new(mailbox: Mailbox<'static>, mailbox_wildcard: ListMailbox<'static>) -> Self {
        Self {
            mailbox,
            mailbox_wildcard,
            output: Vec::new(),
        }
    }
}
