//! A tiny stderr logger (avoids pulling in a logging framework).
//! Level from `NEXSSH_LOG` (error/warn/info/debug/trace); default: info in debug
//! builds, warn in release builds.

use log::{LevelFilter, Log, Metadata, Record};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[{:<5}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let default = if cfg!(debug_assertions) {
        LevelFilter::Info
    } else {
        LevelFilter::Warn
    };
    let level = std::env::var("NEXSSH_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if log::set_logger(&StderrLogger).is_ok() {
        log::set_max_level(level);
    }
}
