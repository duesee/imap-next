use std::num::NonZeroU32;

use imap_types::{
    command::CommandBody,
    datetime::DateTime,
    extensions::binary::LiteralOrLiteral8,
    flag::Flag,
    mailbox::Mailbox,
    response::{Code, StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

#[derive(Clone, Debug)]
pub struct AppendUidTask {
    mailbox: Mailbox<'static>,
    flags: Vec<Flag<'static>>,
    date: Option<DateTime>,
    message: LiteralOrLiteral8<'static>,
}

impl Task for AppendUidTask {
    type Output = Result<Option<(NonZeroU32, NonZeroU32)>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Append {
            mailbox: self.mailbox.clone(),
            flags: self.flags.clone(),
            date: self.date.clone(),
            message: self.message.clone(),
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => {
                if let Some(Code::AppendUid { uid, uid_validity }) = status_body.code {
                    Ok(Some((uid, uid_validity)))
                } else {
                    Ok(None)
                }
            }
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl AppendUidTask {
    pub fn new(mailbox: Mailbox<'static>, message: LiteralOrLiteral8<'static>) -> Self {
        Self {
            mailbox,
            flags: Default::default(),
            date: Default::default(),
            message,
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
