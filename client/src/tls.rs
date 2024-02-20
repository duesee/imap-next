use std::{error::Error, path::Path, sync::Arc};

use tokio::net::TcpStream;
use tokio_rustls::{
    client::TlsStream,
    rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName},
    TlsConnector,
};

pub(crate) async fn do_tls<S: AsRef<Path>>(
    host: &str,
    port: u16,
    additional_trust_anchors: &[S],
) -> Result<TlsStream<TcpStream>, Box<dyn Error>> {
    let stream = TcpStream::connect((host, port)).await?;

    let config = config(additional_trust_anchors).map_err(|_| "failed")?;

    let connector = TlsConnector::from(Arc::new(config));
    let dnsname = ServerName::try_from(host).unwrap();

    Ok(connector.connect(dnsname, stream).await?)
}

// TODO: Error
pub(crate) fn config<S: AsRef<Path>>(additional_trust_anchors: &[S]) -> Result<ClientConfig, ()> {
    let mut root_cert_store = RootCertStore::empty();
    #[allow(deprecated)]
    root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));

    for additional_trust_anchor in additional_trust_anchors {
        let bytes = std::fs::read(additional_trust_anchor.as_ref()).map_err(|_| ())?;
        let (added, ignored) = root_cert_store.add_parsable_certificates(&[bytes]);
        if added != 1 || ignored != 0 {
            return Err(());
        }
    }

    let mut config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"imap".to_vec()];

    Ok(config)
}
