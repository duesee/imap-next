use imap_types::{
    command::CommandBody,
    core::{IString, NString},
    response::{Data, StatusBody, StatusKind},
};

use super::TaskError;
use crate::Task;

pub struct IdTask {
    client: Option<Vec<(IString<'static>, NString<'static>)>>,
    server: Option<Vec<(IString<'static>, NString<'static>)>>,
}

impl Task for IdTask {
    type Output = Result<Option<Vec<(IString<'static>, NString<'static>)>>, TaskError>;

    fn command_body(&self) -> CommandBody<'static> {
        CommandBody::Id {
            parameters: self.client.clone(),
        }
    }

    fn process_data(&mut self, data: Data<'static>) -> Option<Data<'static>> {
        if let Data::Id { parameters } = data {
            self.server = parameters;
            None
        } else {
            Some(data)
        }
    }

    fn process_tagged(self, status_body: StatusBody<'static>) -> Self::Output {
        match status_body.kind {
            StatusKind::Ok => Ok(self.server),
            StatusKind::No => Err(TaskError::UnexpectedNoResponse(status_body)),
            StatusKind::Bad => Err(TaskError::UnexpectedBadResponse(status_body)),
        }
    }
}

impl IdTask {
    pub fn new(parameters: Option<Vec<(IString<'static>, NString<'static>)>>) -> Self {
        Self {
            client: parameters,
            server: None,
        }
    }
}
