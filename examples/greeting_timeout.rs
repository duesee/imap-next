use std::{sync::Arc, time::Duration};

use imap_next::{
    client::{Client as ClientNext, Options},
    stream::Stream,
};
use once_cell::sync::Lazy;
use tokio::{net::TcpStream, time::sleep};
use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConfig, RootCertStore},
    TlsConnector, TlsStream,
};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static ROOT_CERT_STORE: Lazy<RootCertStore> = Lazy::new(|| {
    let mut root_store = RootCertStore::empty();

    for cert in rustls_native_certs::load_native_certs().unwrap() {
        root_store.add(cert).unwrap();
    }

    root_store
});

static HOST: &str = "outlook.office365.com";

/// Run with: RUST_LOG=imap_next=trace cargo run --example greeting_timeout
#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap())
        .with(fmt::layer())
        .init();

    let mut count = 0;

    loop {
        println!("-------------------- next iteration ------------------------------");
        let tcp_stream = TcpStream::connect((HOST, 993)).await.unwrap();

        let mut config = ClientConfig::builder()
            .with_root_certificates(ROOT_CERT_STORE.clone())
            .with_no_client_auth();

        config.alpn_protocols = vec![b"imap".to_vec()];

        let connector = TlsConnector::from(Arc::new(config));
        let dnsname = ServerName::try_from(HOST.to_string()).unwrap();

        let mut stream = Stream::tls(TlsStream::Client(
            connector.connect(dnsname, tcp_stream).await.unwrap(),
        ));

        let mut opts = Options::default();
        opts.crlf_relaxed = true;

        let client_next = ClientNext::new(opts);

        let evt = stream.next(client_next).await;
        println!("evt: {evt:?}");

        sleep(Duration::from_millis(500)).await;

        count += 1;
        println!("=====> {count}");
    }
}
