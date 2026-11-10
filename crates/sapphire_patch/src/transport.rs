//! What a download asks of HTTP, so another client can slot in (m4-plan §3 "HTTP").

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Fetches files from a release's host. [`crate::HttpTransport`] is the real one.
pub trait Transport: Send {
    /// GETs `path`, relative to the host's root. With a `range`, it asks for those bytes and the host must
    /// answer `206 Partial Content`; a `200` is [`FetchError::RangeIgnored`], and the transport stops there
    /// without reading the body. Without one, it asks for the whole file, which must be `200`. The body
    /// goes into `body` as it arrives; when [`Body::write`] fails, the transport stops reading and returns
    /// that error. A body that ends early isn't the transport's to judge: the download compares lengths.
    fn get(&mut self, path: &str, range: Option<Range<u64>>, body: &mut Body<'_>) -> Result<(), FetchError>;
}

/// Why one request failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("the host answered {0}")]
    Status(u16),
    #[error("the host ignored the range asked for and began the whole file")]
    RangeIgnored,
    #[error("the host sent more than the {limit} bytes asked for")]
    TooLong { limit: usize },
    #[error("the host sent {got} of {expected} bytes")]
    Truncated { expected: usize, got: usize },
    #[error("the host sent bytes that don't match their hash or signature")]
    Mismatch,
    #[error("the host went quiet: {0}")]
    Timeout(String),
    #[error("{0}")]
    Network(String),
    #[error("cancelled")]
    Cancelled,
}

impl FetchError {
    /// Whether trying again may help: anything but a cancel or a client error, other than a timeout (408)
    /// or a host asking for a pause (429).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Cancelled => false,
            Self::Status(status) => !(400..500).contains(status) || matches!(status, 408 | 429),
            _ => true,
        }
    }
}

/// Where a response's body goes: at most the bytes the request may have, and none once the download is
/// cancelled.
#[derive(Debug)]
pub struct Body<'a> {
    bytes: Vec<u8>,
    limit: usize,
    cancelled: &'a AtomicBool,
    /// Bytes received from the host, across the download: its wire count.
    received: &'a AtomicU64,
}

impl<'a> Body<'a> {
    pub(crate) fn new(limit: usize, cancelled: &'a AtomicBool, received: &'a AtomicU64) -> Self {
        Self { bytes: Vec::new(), limit, cancelled, received }
    }

    /// Takes the body's next bytes. Fails once the download is cancelled, and when they would take the body
    /// past what the request may have, keeping only what fits.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), FetchError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        let room = self.limit - self.bytes.len();
        let taken = &bytes[..bytes.len().min(room)];
        self.bytes.extend_from_slice(taken);
        self.received.fetch_add(taken.len() as u64, Ordering::Relaxed);
        if bytes.len() > room {
            return Err(FetchError::TooLong { limit: self.limit });
        }
        Ok(())
    }

    /// The most bytes it takes: a range's length, or the most a whole file may be.
    pub fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_takes_no_more_than_its_limit_and_nothing_once_cancelled() {
        let (cancelled, received) = (AtomicBool::new(false), AtomicU64::new(0));
        let mut body = Body::new(10, &cancelled, &received);
        body.write(b"0123456").expect("fits");
        assert_eq!(body.write(b"789ab"), Err(FetchError::TooLong { limit: 10 }));
        assert_eq!(received.load(Ordering::Relaxed), 10);
        assert_eq!(body.into_bytes(), b"0123456789");

        let mut body = Body::new(10, &cancelled, &received);
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(body.write(b"0"), Err(FetchError::Cancelled));
    }

    #[test]
    fn only_a_cancel_and_a_hosts_refusal_stop_the_tries() {
        assert!(!FetchError::Cancelled.is_retryable());
        assert!(!FetchError::Status(404).is_retryable());
        assert!(!FetchError::Status(403).is_retryable());
        for retried in [
            FetchError::Status(503),
            FetchError::Status(429),
            FetchError::Status(408),
            FetchError::RangeIgnored,
            FetchError::Mismatch,
            FetchError::Timeout("read".to_owned()),
        ] {
            assert!(retried.is_retryable(), "{retried}");
        }
    }
}
