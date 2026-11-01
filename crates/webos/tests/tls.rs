//! The TLS port against a server with a throwaway self-signed certificate, the way the TV
//! presents one that names no host.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use common::{UUID, greet, recv};
use rustls::ServerConfig;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use webos::{Endpoint, Error, Fingerprint, Options, Timing, Tls};

/// A TLS server that greets as the TV, and the fingerprint of its certificate.
async fn tv() -> (SocketAddr, Fingerprint) {
    let certified = rcgen::generate_simple_self_signed(["LGE TV SSG".to_owned()]).unwrap();
    let der = certified.cert.der().clone();
    let fingerprint = Fingerprint::of(&der);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![der], key)
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // A client that refuses the certificate ends the handshake here.
                let Ok(stream) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                greet(&mut socket, UUID).await;
                while recv(&mut socket).await.is_some() {}
            });
        }
    });
    (address, fingerprint)
}

fn options(accepted: Vec<Fingerprint>) -> Options {
    Options {
        tls: Tls::accepting(accepted).unwrap(),
        timing: Timing::default(),
    }
}

#[tokio::test]
async fn the_accepted_certificate_connects() {
    let (address, fingerprint) = tv().await;
    let endpoint = Endpoint {
        address,
        secure: true,
    };
    assert!(endpoint.url().starts_with("wss://127.0.0.1:"));
    let options = options(vec![fingerprint]);
    let hello = webos::hello(endpoint, &options).await.unwrap();
    assert_eq!(hello.uuid, UUID);
    assert_eq!(options.tls.seen(), Some(fingerprint));
}

#[tokio::test]
async fn another_certificate_is_refused() {
    let (address, fingerprint) = tv().await;
    let endpoint = Endpoint {
        address,
        secure: true,
    };
    let error = webos::hello(endpoint, &options(vec![webos::LG_TV_CERTIFICATE]))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, Error::UnknownCertificate { found } if found == fingerprint),
        "{error:?}"
    );
}
