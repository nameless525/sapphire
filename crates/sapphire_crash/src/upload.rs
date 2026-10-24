//! Upload of pending reports to Sentry (feature `sentry`).
//!
//! Each run's reports become one Sentry event: a minidump goes up as an `event.minidump`
//! attachment, which Sentry symbolicates into the native crash, and panic reports and Android
//! signal records go up as plain-text attachments, with the panic message on the event.
//!
//! **Transport.** The `sentry` crate is used for the protocol only (event, envelope, DSN). The
//! envelope is posted synchronously with `ureq` so that a run's files are deleted only after
//! Sentry accepted them; `sentry`'s own transports send from a background thread and drop
//! failed envelopes, which would lose reports on every offline launch. TLS is `rustls` with the
//! `ring` provider and bundled `webpki-roots`:
//!
//! - No OpenSSL, which `native-tls` needs on Linux and Android and which must otherwise be
//!   cross-built for Android.
//! - No `aws-lc-rs` (rustls' default provider), whose build wants CMake and, on some hosts, NASM.
//! - `ring` still compiles C and assembly with the `cc` crate, so building for iOS needs Xcode's
//!   clang and building for Android needs the NDK's clang. Both are required to link those
//!   targets anyway, so the feature adds no new toolchain.
//! - The trade-off: certificates are checked against roots compiled into the binary, not the OS
//!   store, so roots update with the crate rather than with the OS. That is fine for Sentry's
//!   public endpoint.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sentry::protocol::{Attachment, AttachmentType, Envelope, Event, Exception, Level, Uuid};
use sentry::types::Dsn;

use crate::report::PanicReport;
use crate::{CrashConfig, CrashError, PendingReport, ReportKind, pending_reports};

const USER_AGENT: &str = concat!("sapphire_crash/", env!("CARGO_PKG_VERSION"));
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// What [`upload_pending_reports`] did, counted in runs (one Sentry event each).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadSummary {
    /// Runs Sentry accepted. Their files have been deleted.
    pub uploaded: usize,
    /// Runs that could not be sent. Their files stay for the next attempt.
    pub failed: usize,
}

/// Uploads every pending report in [`CrashConfig::dump_dir`] to [`CrashConfig::upload_dsn`],
/// deleting each run's files once Sentry has accepted them.
///
/// This blocks on the network, so call it from a background thread or job at startup. A run
/// that fails to upload (offline, rate-limited, server error) is logged and kept for next time.
///
/// # Errors
///
/// [`CrashError::NoUploadDsn`] or [`CrashError::InvalidDsn`] if the DSN is missing or malformed,
/// and [`CrashError::ReadDumpDir`] if the dump directory cannot be listed.
pub fn upload_pending_reports(config: &CrashConfig) -> Result<UploadSummary, CrashError> {
    let dsn: Dsn = config
        .upload_dsn
        .as_deref()
        .ok_or(CrashError::NoUploadDsn)?
        .parse()
        .map_err(|err: sentry::types::ParseDsnError| CrashError::InvalidDsn(err.to_string()))?;

    let mut runs: BTreeMap<String, Vec<PendingReport>> = BTreeMap::new();
    for report in pending_reports(&config.dump_dir)? {
        runs.entry(report.run_id().to_owned()).or_default().push(report);
    }
    if runs.is_empty() {
        return Ok(UploadSummary::default());
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(UPLOAD_TIMEOUT))
        .user_agent(USER_AGENT)
        .build()
        .into();
    let url = dsn.envelope_api_url();
    let auth = dsn.to_auth(Some(USER_AGENT)).to_string();

    let mut summary = UploadSummary::default();
    for (run_id, reports) in runs {
        let sent = build_envelope(config, &run_id, &reports)
            .map_err(SendError::Read)
            .and_then(|envelope| send(&agent, url.as_str(), &auth, &envelope));
        match sent {
            Ok(()) => {
                summary.uploaded += 1;
                for report in &reports {
                    if let Err(err) = fs::remove_file(report.path()) {
                        tracing::warn!(
                            target: "sapphire::crash",
                            "uploaded {} but could not delete it: {err}",
                            report.path().display(),
                        );
                    }
                }
            }
            Err(err) => {
                summary.failed += 1;
                tracing::warn!(
                    target: "sapphire::crash",
                    run = %run_id,
                    "crash report upload failed, keeping it for the next attempt: {err}",
                );
            }
        }
    }
    tracing::info!(
        target: "sapphire::crash",
        uploaded = summary.uploaded,
        failed = summary.failed,
        "crash report upload finished",
    );
    Ok(summary)
}

#[derive(Debug, thiserror::Error)]
enum SendError {
    #[error("cannot read the reports: {0}")]
    Read(io::Error),
    #[error("cannot serialize the envelope: {0}")]
    Serialize(io::Error),
    #[error("{0}")]
    Http(Box<ureq::Error>),
}

fn send(agent: &ureq::Agent, url: &str, auth: &str, envelope: &Envelope) -> Result<(), SendError> {
    let mut body = Vec::new();
    envelope.to_writer(&mut body).map_err(SendError::Serialize)?;
    // Non-2xx statuses come back as `ureq::Error::StatusCode`, so `Ok` means Sentry took it.
    agent
        .post(url)
        .header("X-Sentry-Auth", auth)
        .header("Content-Type", "application/x-sentry-envelope")
        .send(&body[..])
        .map_err(|err| SendError::Http(Box::new(err)))?;
    Ok(())
}

/// A Sentry event id derived from the run, so re-uploading a run after a lost response updates
/// the same issue instead of creating a duplicate. We send one event per run, so the run id is
/// the dedup key; it is hashed into 16 bytes (FNV-1a) shaped to look like a UUID.
fn event_id_for_run(run_id: &str) -> Uuid {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for byte in run_id.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    // Set the UUID version (4) and variant (RFC 4122) nibbles so it is a well-formed UUID.
    let bytes = hash.to_be_bytes();
    let mut out = bytes;
    out[6] = (bytes[6] & 0x0f) | 0x40;
    out[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(out)
}

/// Builds the envelope for one run: an event plus one attachment per report.
fn build_envelope(config: &CrashConfig, run_id: &str, reports: &[PendingReport]) -> io::Result<Envelope> {
    let mut event = Event {
        event_id: event_id_for_run(run_id),
        level: Level::Fatal,
        platform: "native".into(),
        release: Some(config.release.clone().into()),
        ..Event::default()
    };
    event.tags.insert("app".into(), config.app_name.clone());
    event.tags.insert("sapphire.run".into(), run_id.to_owned());

    let mut attachments = Vec::with_capacity(reports.len());
    let mut has_minidump = false;
    let mut panic = None;
    let mut signal_line = None;
    let mut crash_time: Option<SystemTime> = None;
    for report in reports {
        let buffer = fs::read(report.path())?;
        crash_time = crash_time.or_else(|| fs::metadata(report.path()).and_then(|m| m.modified()).ok());
        let filename =
            report.path().file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let ty = match report.kind() {
            ReportKind::Minidump => {
                has_minidump = true;
                AttachmentType::Minidump
            }
            ReportKind::Panic => {
                if panic.is_none() {
                    panic = PanicReport::parse(&String::from_utf8_lossy(&buffer));
                }
                AttachmentType::Attachment
            }
            ReportKind::NativeSignal => {
                signal_line = String::from_utf8_lossy(&buffer)
                    .lines()
                    .find_map(|line| line.strip_prefix("signal: ").map(str::to_owned));
                AttachmentType::Attachment
            }
        };
        let content_type = match ty {
            AttachmentType::Minidump => "application/octet-stream",
            _ => "text/plain",
        };
        attachments.push(Attachment {
            buffer,
            filename,
            content_type: Some(content_type.into()),
            ty: Some(ty),
        });
    }

    if let Some(panic) = panic {
        event.timestamp = UNIX_EPOCH + Duration::from_millis(panic.unix_time_ms);
        event.tags.insert("panic.location".into(), panic.location);
        event.tags.insert("panic.thread".into(), panic.thread);
        // With a minidump, Sentry derives the exception from it; the panic stays the message.
        if !has_minidump {
            event.exception = vec![Exception {
                ty: "panic".into(),
                value: Some(panic.message.clone()),
                ..Exception::default()
            }]
            .into();
        }
        event.message = Some(panic.message);
    } else {
        if let Some(time) = crash_time {
            event.timestamp = time;
        }
        if let Some(signal) = signal_line {
            event.message = Some(format!("native crash: {signal}"));
        }
    }

    let mut envelope = Envelope::new();
    envelope.add_item(event);
    for attachment in attachments {
        envelope.add_item(attachment);
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use sentry::protocol::EnvelopeItem;

    use super::*;
    use crate::report;
    use crate::test_support::ScratchDir;

    fn config(dir: &ScratchDir) -> CrashConfig {
        CrashConfig::new("upload-test", "9.9.9", dir.path())
            .with_upload_dsn("https://public@sentry.example.invalid/42")
    }

    fn panic_report(run_id: &str) -> PanicReport {
        PanicReport {
            app_name: "upload-test".into(),
            release: "9.9.9".into(),
            run_id: run_id.into(),
            pid: 7,
            thread: "main (ThreadId(1))".into(),
            location: "src/main.rs:1:1".into(),
            unix_time_ms: 1_761_300_000_000,
            uptime_ms: 5,
            modules: Vec::new(),
            frames: Vec::new(),
            message: "boom".into(),
            backtrace: "   0: main\n".into(),
        }
    }

    #[test]
    fn event_id_is_deterministic_per_run() {
        let run = report::new_run_id(1_761_300_000_000, 7);
        // Stable across calls, so a retry hits the same Sentry issue.
        assert_eq!(event_id_for_run(&run), event_id_for_run(&run));
        // Different runs get different ids.
        assert_ne!(event_id_for_run(&run), event_id_for_run(&report::new_run_id(1_761_300_000_000, 8)));
        // Well-formed UUIDv4 shape.
        assert_eq!(event_id_for_run(&run).get_version_num(), 4);
    }

    #[test]
    fn minidump_and_panic_make_one_event() {
        let dir = ScratchDir::new("upload-envelope");
        let run_id = report::new_run_id(1_761_300_000_000, 7);
        fs::write(dir.path().join(report::minidump_file_name(&run_id)), b"MDMP\x93\xa7fake").expect("write");
        let text = panic_report(&run_id).render();
        fs::write(dir.path().join(report::panic_file_name(&run_id, 0)), text).expect("write");

        let config = config(&dir);
        let reports = pending_reports(dir.path()).expect("list");
        let envelope = build_envelope(&config, &run_id, &reports).expect("envelope");

        let event = envelope.event().expect("event");
        assert_eq!(event.level, Level::Fatal);
        assert_eq!(event.message.as_deref(), Some("boom"));
        assert_eq!(event.release.as_deref(), Some("9.9.9"));
        assert!(event.exception.is_empty(), "the minidump provides the exception");
        assert_eq!(event.tags.get("sapphire.run"), Some(&run_id));

        let attachments: Vec<_> = envelope
            .items()
            .filter_map(|item| match item {
                EnvelopeItem::Attachment(a) => Some((a.ty.clone(), a.filename.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            attachments,
            [
                (Some(AttachmentType::Minidump), report::minidump_file_name(&run_id)),
                (Some(AttachmentType::Attachment), report::panic_file_name(&run_id, 0)),
            ]
        );

        let mut bytes = Vec::new();
        envelope.to_writer(&mut bytes).expect("serialize");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(r#""attachment_type":"event.minidump""#), "{text}");
    }

    #[test]
    fn panic_without_minidump_becomes_an_exception() {
        let dir = ScratchDir::new("upload-panic-only");
        let run_id = report::new_run_id(1_761_300_000_000, 8);
        fs::write(dir.path().join(report::panic_file_name(&run_id, 0)), panic_report(&run_id).render())
            .expect("write");
        let reports = pending_reports(dir.path()).expect("list");
        let envelope = build_envelope(&config(&dir), &run_id, &reports).expect("envelope");
        let event = envelope.event().expect("event");
        assert_eq!(event.exception.len(), 1);
        assert_eq!(event.exception[0].value.as_deref(), Some("boom"));
        assert_eq!(event.timestamp, UNIX_EPOCH + Duration::from_millis(1_761_300_000_000));
    }

    #[test]
    fn signal_record_names_the_signal() {
        let dir = ScratchDir::new("upload-signal");
        let run_id = report::new_run_id(1_761_300_000_000, 9);
        let record = format!("Sapphire native crash record v1\nrun: {run_id}\nsignal: 11 (SIGSEGV)\n");
        fs::write(dir.path().join(report::native_file_name(&run_id)), record).expect("write");
        let reports = pending_reports(dir.path()).expect("list");
        let envelope = build_envelope(&config(&dir), &run_id, &reports).expect("envelope");
        assert_eq!(envelope.event().and_then(|e| e.message.as_deref()), Some("native crash: 11 (SIGSEGV)"));
    }

    #[test]
    fn rejects_missing_or_bad_dsn() {
        let dir = ScratchDir::new("upload-dsn");
        let mut config = config(&dir);
        config.upload_dsn = None;
        assert!(matches!(upload_pending_reports(&config), Err(CrashError::NoUploadDsn)));
        config.upload_dsn = Some("not a dsn".into());
        assert!(matches!(upload_pending_reports(&config), Err(CrashError::InvalidDsn(_))));
    }

    #[test]
    fn nothing_pending_means_no_network() {
        let dir = ScratchDir::new("upload-empty");
        assert_eq!(upload_pending_reports(&config(&dir)).expect("ok"), UploadSummary::default());
    }
}
