//! The real transport: `ureq` 3, blocking, on `rustls` with the `ring` provider (m4-plan §3 "HTTP").

use std::io::{self, Read};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use sapphire_platform::Os;
use ureq::Agent;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

use crate::transport::{Body, FetchError, Transport};

/// HTTP to one host, by `ureq` (see the [crate docs](crate)). Each request is one GET, on connections it
/// keeps open between them.
#[derive(Debug)]
pub struct HttpTransport {
    agent: Agent,
    base: String,
}

/// Which certificates a host's must chain to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Roots {
    /// The OS's own verifier and trust store (`rustls-platform-verifier`), everywhere but Android, where it's
    /// Mozilla's roots, compiled in (`webpki-roots`): the platform verifier's Android half needs a Kotlin
    /// component from outside its crate, and Gradle builds offline here. Android then trusts no CA a user
    /// installed, and its roots age until an app update.
    Platform,
    /// These certificates only, DER-encoded: a host with its own CA, and tests.
    Specific(Vec<Vec<u8>>),
}

/// How long a request may wait. A read that waits longer counts as a stall, and is tried again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// To connect, TLS's handshake included.
    pub connect: Duration,
    /// From the request to the response's headers.
    pub response: Duration,
    /// For each read of the body, so a slow host that keeps sending isn't cut off.
    pub read: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            response: Duration::from_secs(20),
            read: Duration::from_secs(15),
        }
    }
}

/// A host that can't be reached through this transport.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("a release's host is an http:// or https:// URL ending in `/`, not {0:?}")]
    BadBase(String),
}

impl HttpTransport {
    /// A transport to the release laid out under `base`, an `http://` or `https://` URL ending in `/`: where
    /// `release.smanifest` and the `packs/` and `files/` directories are (`xtask release`'s layout).
    pub fn new(base: &str, roots: Roots, timeouts: Timeouts) -> Result<Self, HttpError> {
        let web = base.starts_with("https://") || base.starts_with("http://");
        if !web || !base.ends_with('/') {
            return Err(HttpError::BadBase(base.to_owned()));
        }
        let root_certs = match roots {
            // The manifest gives `ureq` the matching feature on each target: the platform verifier on all but
            // Android, Mozilla's roots on Android.
            Roots::Platform if Os::CURRENT == Os::Android => RootCerts::WebPki,
            Roots::Platform => RootCerts::PlatformVerifier,
            Roots::Specific(certificates) => RootCerts::new_with_certs(
                &certificates.iter().map(|der| Certificate::from_der(der).to_owned()).collect::<Vec<_>>(),
            ),
        };
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(root_certs)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = Agent::config_builder()
            .tls_config(tls)
            // Statuses are the download's to judge: a 200 to a range request isn't an error to `ureq`.
            .http_status_as_error(false)
            .timeout_connect(Some(timeouts.connect))
            .timeout_recv_response(Some(timeouts.response))
            .timeout_per_read(Some(timeouts.read))
            .build()
            .new_agent();
        Ok(Self { agent, base: base.to_owned() })
    }
}

impl Transport for HttpTransport {
    fn get(&mut self, path: &str, range: Option<Range<u64>>, body: &mut Body<'_>) -> Result<(), FetchError> {
        let mut request = self.agent.get(format!("{}{path}", self.base));
        if let Some(range) = &range {
            // Ranges are inclusive; the download never asks for an empty one.
            request = request.header("Range", format!("bytes={}-{}", range.start, range.end - 1));
        }
        let response = request.call().map_err(fetch_error)?;
        match (range.is_some(), response.status().as_u16()) {
            (true, 206) | (false, 200) => {}
            // Dropping the response closes its connection without reading the file.
            (true, 200) => return Err(FetchError::RangeIgnored),
            (_, status) => return Err(FetchError::Status(status)),
        }
        let mut reader = response.into_body().into_reader();
        let mut chunk = [0; 16 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => return Ok(()),
                Ok(read) => body.write(&chunk[..read])?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(io_fetch_error(error)),
            }
        }
    }
}

fn fetch_error(error: ureq::Error) -> FetchError {
    match error {
        ureq::Error::Timeout(timeout) => FetchError::Timeout(timeout.to_string()),
        ureq::Error::Io(error) => io_fetch_error(error),
        error => FetchError::Network(error.to_string()),
    }
}

/// A body's read error: `ureq`'s own, wrapped, or the socket's.
fn io_fetch_error(error: io::Error) -> FetchError {
    let error = match error.downcast::<ureq::Error>() {
        Ok(inner) => return fetch_error(inner),
        Err(error) => error,
    };
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => FetchError::Timeout(error.to_string()),
        _ => FetchError::Network(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_a_web_url_ending_in_a_slash() {
        for base in ["https://patches.example/game/", "http://127.0.0.1:8080/"] {
            assert!(HttpTransport::new(base, Roots::Platform, Timeouts::default()).is_ok(), "{base}");
        }
        for base in ["https://patches.example/game", "ftp://patches.example/", "patches.example/", ""] {
            assert_eq!(
                HttpTransport::new(base, Roots::Platform, Timeouts::default()).unwrap_err(),
                HttpError::BadBase(base.to_owned())
            );
        }
    }
}
