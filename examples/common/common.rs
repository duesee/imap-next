use std::sync::Arc;

use tokio::net::TcpStream;
use tokio_rustls::{
    client::TlsStream as TokioTlsStream,
    rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName},
    TlsConnector,
};

pub(crate) struct TlsStream {}

impl TlsStream {
    pub(crate) async fn connect(host: &str, port: u16) -> TokioTlsStream<TcpStream> {
        let stream = TcpStream::connect((host, port)).await.unwrap();

        let config = {
            let root_store = {
                let mut root_store = RootCertStore::empty();

                root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
                    OwnedTrustAnchor::from_subject_spki_name_constraints(
                        ta.subject,
                        ta.spki,
                        ta.name_constraints,
                    )
                }));

                root_store
            };

            let mut config = ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_store)
                .with_no_client_auth();

            // See <https://www.iana.org/assignments/tls-extensiontype-values/tls-extensiontype-values.xhtml#alpn-protocol-ids>
            config.alpn_protocols = vec![b"imap".to_vec()];

            config
        };

        let connector = TlsConnector::from(Arc::new(config));
        let dnsname = ServerName::try_from(host).unwrap();

        connector.connect(dnsname, stream).await.unwrap()
    }
}
