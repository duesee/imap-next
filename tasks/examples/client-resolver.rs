use imap_flow::{
    client::{ClientFlow, ClientFlowOptions},
    stream::Stream,
};
use tasks::{
    resolver::Resolver,
    tasks::{authenticate::AuthenticateTask, capability::CapabilityTask, logout::LogoutTask},
};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::insecure(stream);
    let client = ClientFlow::new(ClientFlowOptions::default());
    let mut resolver = Resolver::new(client);

    let capability = stream
        .progress(resolver.run_task(CapabilityTask::new()))
        .await
        .unwrap()
        .unwrap();

    println!("pre-auth capability: {capability:?}");

    let capability = stream
        .progress(resolver.run_task(AuthenticateTask::plain("alice", "pa²²w0rd", true)))
        .await
        .unwrap()
        .unwrap();

    println!("maybe post-auth capability: {capability:?}");

    let capability = stream
        .progress(resolver.run_task(CapabilityTask::new()))
        .await
        .unwrap()
        .unwrap();

    println!("post-auth capability: {capability:?}");

    stream
        .progress(resolver.run_task(LogoutTask::default()))
        .await
        .unwrap()
        .unwrap();
}
