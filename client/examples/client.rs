use imap_client::Client;

#[tokio::main]
async fn main() {
    let mut client = Client::insecure("127.0.0.1", 12345).await.unwrap();

    let capabilities = client.refresh_capabilities().await.unwrap();

    println!("capabilities: {capabilities:?}");
}
