use imap_flow::{client::ClientFlowOptions, stream::AnyStream};
use imap_types::response::{Response, Status};
use tasks::{
    tasks::{authenticate::AuthenticateTask, capability::CapabilityTask, logout::LogoutTask},
    Scheduler, SchedulerEvent,
};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let mut scheduler = {
        let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
        Scheduler::try_new_flow(AnyStream::new(stream), ClientFlowOptions::default())
            .await
            .unwrap()
    };

    let handle1 = scheduler.enqueue_task(CapabilityTask::default());

    loop {
        match scheduler.progress().await.unwrap() {
            SchedulerEvent::TaskFinished(mut token) => {
                if let Some(capability) = handle1.resolve(&mut token) {
                    println!("handle1: {capability:?}");
                    break;
                }
            }
            SchedulerEvent::Unsolicited(unsolicited) => {
                println!("unsolicited: {unsolicited:?}");

                if let Response::Status(Status::Bye { .. }) = unsolicited {
                    break;
                }
            }
        }
    }

    let handle2 = scheduler.enqueue_task(AuthenticateTask::new_plain("alice", "pa²²w0rd", true));
    let handle3 = scheduler.enqueue_task(LogoutTask::default());

    loop {
        match scheduler.progress().await.unwrap() {
            SchedulerEvent::TaskFinished(mut token) => {
                if let Some(auth) = handle2.resolve(&mut token) {
                    println!("handle2: {auth:?}");
                }

                if let Some(logout) = handle3.resolve(&mut token) {
                    println!("handle3: {logout:?}");
                    break;
                }
            }
            SchedulerEvent::Unsolicited(unsolicited) => {
                println!("unsolicited: {unsolicited:?}");

                if let Response::Status(Status::Bye { .. }) = unsolicited {
                    break;
                }
            }
        }
    }
}
