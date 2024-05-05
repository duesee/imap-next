use imap_flow::{
    client::{ClientFlow, ClientFlowOptions},
    stream::Stream,
};
use tasks::{resolver::Resolver, Scheduler};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:12345").await.unwrap();
    let mut stream = Stream::insecure(stream);

    let client = ClientFlow::new(ClientFlowOptions::default());
    let scheduler = Scheduler::new(client);
    let mut resolver = Resolver::new(scheduler);

    let handle = resolver.capability();
    let capabilities = stream
        .progress(&mut resolver)
        .await
        .unwrap()
        .resolve(handle)
        .unwrap();

    println!("capabilities: {capabilities:?}");

    let handle = resolver.authenticate_plain("alice", "pa²²w0rd");
    stream
        .progress(&mut resolver)
        .await
        .unwrap()
        .resolve(handle)
        .unwrap();

    let handle = resolver.logout();
    stream
        .progress(&mut resolver)
        .await
        .unwrap()
        .resolve(handle)
        .unwrap();
}
