use std::sync::Arc;

use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use crate::error::{BotError, BotResult};

pub async fn connect(host: &str, port: u16) -> BotResult<TlsStream<TcpStream>> {
    let connector = connector();
    let tcp = TcpStream::connect((host, port)).await?;
    tcp.set_nodelay(true).ok();

    let domain = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| BotError::Tls(e.to_string()))?;
    Ok(connector.connect(domain, tcp).await?)
}

fn connector() -> TlsConnector {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("aws-lc-rs supports the default protocol versions")
        .with_root_certificates(root_store)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}
