use imap_flow::{
    client::{ClientFlow, ClientFlowOptions},
    stream::Stream,
};
use imap_types::response::{Response, Status};
use tasks::{
    tasks::{authenticate::AuthenticateTask, capability::CapabilityTask, logout::LogoutTask},
    Scheduler, SchedulerEvent,
};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::insecure(stream);
    let client = ClientFlow::new(ClientFlowOptions::default());
    let mut scheduler = Scheduler::new(client);

    let capability_handle = scheduler.enqueue_task(CapabilityTask::default());

    loop {
        match stream.progress(&mut scheduler).await.unwrap() {
            SchedulerEvent::GreetingReceived(greeting) => {
                println!("greeting: {greeting:?}");
            }
            SchedulerEvent::TaskFinished(mut token) => {
                if let Some(capability) = capability_handle.resolve(&mut token) {
                    println!("capability: {capability:?}");
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

    let auth_handle = scheduler.enqueue_task(AuthenticateTask::plain("alice", "pa²²w0rd", true));
    let logout_handle = scheduler.enqueue_task(LogoutTask::default());

    loop {
        match stream.progress(&mut scheduler).await.unwrap() {
            SchedulerEvent::GreetingReceived(_) => {
                unreachable!()
            }
            SchedulerEvent::TaskFinished(mut token) => {
                if let Some(auth) = auth_handle.resolve(&mut token) {
                    println!("auth: {auth:?}");
                }

                if let Some(logout) = logout_handle.resolve(&mut token) {
                    println!("logout: {logout:?}");
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
