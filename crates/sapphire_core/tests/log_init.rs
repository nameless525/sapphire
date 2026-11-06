//! `log::init` installs process-global state, so it gets its own test binary with one test.

use std::sync::{Arc, Mutex};

use sapphire_core::log::{self, LogConfig, LogInitError, LogLevel, LogRecord};

#[test]
fn init_once_then_already_initialized() {
    if std::env::var_os("SAPPHIRE_LOG_INIT_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "init_once_then_already_initialized", "--nocapture"])
            .env("SAPPHIRE_LOG_INIT_CHILD", "1")
            .env("SAPPHIRE_LOG", "error")
            .env_remove("RUST_LOG")
            .status()
            .expect("run isolated logger test");
        assert!(status.success(), "isolated logger test failed: {status}");
        return;
    }
    let records: Arc<Mutex<Vec<(LogLevel, String, String)>>> = Arc::default();
    let sink_records = Arc::clone(&records);
    let sink = move |record: &LogRecord<'_>| {
        sink_records.lock().expect("test mutex").push((
            record.level,
            record.target.to_owned(),
            record.message.to_owned(),
        ));
    };
    // The child owns the logging filter without mutating the test runner environment.
    let config = LogConfig { stderr: false, ..LogConfig::default() };
    log::init(config.clone(), Some(Box::new(sink))).expect("first init succeeds");

    tracing::error!(target: "sapphire::test", code = 7, "boom");

    let second = log::init(config, None);
    assert!(matches!(second, Err(LogInitError::AlreadyInitialized)), "{second:?}");

    let records = records.lock().expect("test mutex");
    assert!(
        records.contains(&(LogLevel::Error, "sapphire::test".to_owned(), "boom code=7".to_owned())),
        "{records:?}"
    );
}
