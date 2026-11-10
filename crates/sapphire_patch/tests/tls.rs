//! The download over TLS, against a localhost host with a certificate made for the test: trusted as a
//! specific root it serves the patch; the OS's verifier refuses it, since no trust store holds it. Nothing
//! is installed in a trust store, and the certificate isn't kept.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::server::Host;
use common::{
    APP_BUILD, EXPIRES, NOW, TRUSTED, TestDir, assert_staged_whole, first_release, retries, second_release,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sapphire_patch::{
    Download, DownloadError, FetchError, HttpTransport, MANIFEST_PATH, Outcome, Request, Roots, Timeouts,
};

/// A host serving release 2 over TLS, and its certificate's DER.
fn tls_host() -> (Host, Vec<u8>) {
    let certified =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("makes a certificate");
    let der = certified.cert.der().to_vec();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()));
    let config =
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("TLS 1.2 and 1.3")
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(der.clone())], key)
            .expect("a certificate and its key");
    let next = second_release();
    (Host::start_tls(next.host(&next.signed(2, EXPIRES, TRUSTED)), config), der)
}

fn download(host: &Host, roots: Roots, dir: &TestDir) -> Result<Outcome, DownloadError> {
    let request = Request {
        installed: first_release().installed(1),
        staging: dir.0.clone(),
        now: NOW,
        app_build: APP_BUILD,
        retries: retries(),
    };
    let quick = Duration::from_secs(5);
    let timeouts = Timeouts { connect: quick, response: quick, read: quick };
    Download::start(request, HttpTransport::new(&host.url(), roots, timeouts).expect("a host"))
        .expect("starts")
        .join()
}

#[test]
fn a_host_whose_certificate_chains_to_a_trusted_root_serves_the_patch() {
    let (host, der) = tls_host();
    let dir = TestDir::new("tls-trusted");
    let outcome = download(&host, Roots::Specific(vec![der]), &dir).expect("downloads");
    assert!(matches!(outcome, Outcome::Staged(_)), "{outcome:?}");
    assert_staged_whole(&dir.0, &first_release(), &second_release());
}

#[test]
fn the_platform_verifier_refuses_a_certificate_no_trust_store_holds() {
    let (host, _) = tls_host();
    let dir = TestDir::new("tls-platform");
    let error = download(&host, Roots::Platform, &dir).expect_err("refused");
    match &error {
        DownloadError::Host { path, last: FetchError::Network(message), .. } => {
            assert_eq!(path, MANIFEST_PATH);
            assert!(message.to_ascii_lowercase().contains("certificate"), "{message}");
        }
        other => panic!("{other}"),
    }
    assert!(host.log().is_empty(), "no request got past the handshake");
}
