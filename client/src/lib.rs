#![forbid(unsafe_code)]

pub(crate) mod tls;

use std::{error::Error, fmt::Debug, path::Path};

use imap_flow::{
    client::{ClientFlow, ClientFlowOptions},
    stream::AnyStream,
};
use imap_types::{
    fetch::MacroOrMessageDataItemNames,
    mailbox::Mailbox,
    response::{Bye, Greeting, Response, StatusBody},
    sequence::SequenceSet,
};
use tasks::{
    tasks::{Fetch, FetchTask, LoginTask, LogoutTask, SelectTask, Session},
    Scheduler, SchedulerEvent,
};
use tracing::instrument;

pub struct Client {
    scheduler: Scheduler,
    unsolicited: Vec<Response<'static>>,
}

impl Client {
    #[instrument]
    pub async fn receive_greeting(
        host: &str,
        port: u16,
    ) -> Result<(Greeting<'static>, Self), Box<dyn Error>> {
        let stream = tls::do_tls::<&str>(host, port, &[]).await?;
        let (flow, greeting) =
            ClientFlow::receive_greeting(AnyStream::new(stream), ClientFlowOptions::default())
                .await?;

        Ok((
            greeting,
            Self {
                scheduler: Scheduler::new(flow),
                unsolicited: Vec::default(),
            },
        ))
    }

    #[instrument(skip(self))]
    pub async fn login(
        &mut self,
        username: String,
        password: String,
    ) -> Result<StatusBody<'static>, Box<dyn Error>> {
        let task = self
            .scheduler
            .enqueue_task(LoginTask::new(username, password));

        loop {
            match self.scheduler.progress().await? {
                SchedulerEvent::TaskFinished(mut token) => {
                    break if let Some(data) = task.resolve(&mut token) {
                        Ok(data)
                    } else {
                        Err("meh".into())
                    };
                }
                SchedulerEvent::Unsolicited(response) => {
                    self.unsolicited.push(response);
                }
            }
        }
    }

    #[instrument(skip(self))]
    pub async fn select(
        &mut self,
        mailbox: Mailbox<'static>,
    ) -> Result<(Session, StatusBody<'static>), Box<dyn Error>> {
        let task = self.scheduler.enqueue_task(SelectTask::new(mailbox));

        loop {
            match self.scheduler.progress().await? {
                SchedulerEvent::TaskFinished(mut token) => {
                    break if let Some(data) = task.resolve(&mut token) {
                        Ok(data)
                    } else {
                        Err("meh".into())
                    };
                }
                SchedulerEvent::Unsolicited(response) => {
                    self.unsolicited.push(response);
                }
            }
        }
    }

    #[instrument(skip(self))]
    pub async fn fetch<S, I>(
        &mut self,
        sequence_set: S,
        macro_or_data_item: I,
        uid: bool,
    ) -> Result<(Vec<Fetch>, StatusBody<'static>), Box<dyn Error>>
    where
        S: TryInto<SequenceSet> + Debug,
        // TODO
        S::Error: Debug,
        I: Into<MacroOrMessageDataItemNames<'static>> + Debug,
    {
        let task =
            self.scheduler
                .enqueue_task(FetchTask::new(sequence_set, macro_or_data_item, uid));

        loop {
            match self.scheduler.progress().await? {
                SchedulerEvent::TaskFinished(mut token) => {
                    break if let Some(data) = task.resolve(&mut token) {
                        Ok(data)
                    } else {
                        Err("meh".into())
                    };
                }
                SchedulerEvent::Unsolicited(response) => {
                    self.unsolicited.push(response);
                }
            }
        }
    }

    #[instrument(skip(self))]
    pub async fn logout(
        &mut self,
    ) -> Result<(Option<Bye<'static>>, StatusBody<'static>), Box<dyn Error>> {
        let task = self.scheduler.enqueue_task(LogoutTask::new());

        loop {
            match self.scheduler.progress().await? {
                SchedulerEvent::TaskFinished(mut token) => {
                    break if let Some(data) = task.resolve(&mut token) {
                        Ok(data)
                    } else {
                        Err("meh".into())
                    };
                }
                SchedulerEvent::Unsolicited(response) => {
                    self.unsolicited.push(response);
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct ClientBuilder<'a, S> {
    additional_trust_anchors: &'a [S],
}

impl<'a, S> ClientBuilder<'a, S>
where
    S: AsRef<Path>,
{
    pub fn new() -> Self {
        Self {
            additional_trust_anchors: &[],
        }
    }

    pub fn additional_trust_anchors(mut self, additional_trust_anchors: &'a [S]) -> Self {
        self.additional_trust_anchors = additional_trust_anchors;
        self
    }

    pub async fn receive_greeting(
        self,
        host: &str,
        port: u16,
    ) -> Result<(Greeting<'static>, Client), Box<dyn Error>> {
        let stream = tls::do_tls(host, port, self.additional_trust_anchors).await?;
        let (flow, greeting) =
            ClientFlow::receive_greeting(AnyStream::new(stream), ClientFlowOptions::default())
                .await?;

        Ok((
            greeting,
            Client {
                scheduler: Scheduler::new(flow),
                unsolicited: Vec::default(),
            },
        ))
    }
}
