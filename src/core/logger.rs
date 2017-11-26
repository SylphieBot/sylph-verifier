use chrono::Local;
use linefeed::reader::LogSender;
use log::*;
use parking_lot::Mutex;
use std::cmp::max;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering, ATOMIC_BOOL_INIT};

struct AtomicLogLevel(AtomicU8);
impl AtomicLogLevel {
    const LOG_OFF: u8 = 0;
    const LOG_ERROR: u8 = 1;
    const LOG_WARN: u8 = 2;
    const LOG_INFO: u8 = 3;
    const LOG_DEBUG: u8 = 4;
    const LOG_TRACE: u8 = 5;

    fn load(&self) -> LogLevelFilter {
        match self.0.load(Ordering::Relaxed) {
            Self::LOG_OFF   => LogLevelFilter::Off,
            Self::LOG_ERROR => LogLevelFilter::Error,
            Self::LOG_WARN  => LogLevelFilter::Warn,
            Self::LOG_INFO  => LogLevelFilter::Info,
            Self::LOG_DEBUG => LogLevelFilter::Debug,
            Self::LOG_TRACE => LogLevelFilter::Trace,
            _ => unreachable!(),
        }
    }
    fn store(&self, level: LogLevelFilter) {
        self.0.store(match level {
            LogLevelFilter::Off   => Self::LOG_OFF,
            LogLevelFilter::Error => Self::LOG_ERROR,
            LogLevelFilter::Warn  => Self::LOG_WARN,
            LogLevelFilter::Info  => Self::LOG_INFO,
            LogLevelFilter::Debug => Self::LOG_DEBUG,
            LogLevelFilter::Trace => Self::LOG_TRACE,
        }, Ordering::Relaxed)
    }
    fn logs(&self, level: LogLevel) -> bool {
        match self.load().to_log_level() {
            Some(filter) => level <= filter,
            None => false,
        }
    }
}

static LOG_LEVEL_APP: AtomicLogLevel = AtomicLogLevel(AtomicU8::new(AtomicLogLevel::LOG_INFO));
static LOG_LEVEL_LIB: AtomicLogLevel = AtomicLogLevel(AtomicU8::new(AtomicLogLevel::LOG_WARN));
static LOG_LEVEL_FILTER: Mutex<Option<MaxLogLevelFilter>> = Mutex::new(None);

fn update_log_filter_obj() {
    LOG_LEVEL_FILTER.lock().as_ref().unwrap().set(max(LOG_LEVEL_APP.load(), LOG_LEVEL_LIB.load()))
}
fn set_log_filter_obj(filter: MaxLogLevelFilter) {
    *LOG_LEVEL_FILTER.lock() = Some(filter);
    update_log_filter_obj();
}
pub fn set_app_filter_level(level: LogLevelFilter) {
    LOG_LEVEL_APP.store(level);
    update_log_filter_obj();
}
pub fn set_lib_filter_level(level: LogLevelFilter) {
    LOG_LEVEL_LIB.store(level);
    update_log_filter_obj();
}
fn is_app_source(source: &str) -> bool {
    source == "sylph_verifier" || source.starts_with("sylph_verifier::")
}

static LOG_SENDER_EXISTS: AtomicBool = ATOMIC_BOOL_INIT;
static RAW_LOG_SENDER: Mutex<Option<LogSender>> = Mutex::new(None);
thread_local!(static LOG_SENDER: LogSender = RAW_LOG_SENDER.lock().as_ref().unwrap().clone());

pub fn set_log_sender(sender: LogSender) {
    {
        let mut cur_sender = RAW_LOG_SENDER.lock();
        if cur_sender.is_some() {
            panic!("Attempted to set log sender twice!")
        }
        *cur_sender = Some(sender);
    }
    LOG_SENDER_EXISTS.store(true, Ordering::Release)
}

struct Logger;
impl Logger {
    fn check_level(&self, source: &str, level: LogLevel) -> bool {
        if is_app_source(source) {
            LOG_LEVEL_APP.logs(level)
        } else {
            LOG_LEVEL_LIB.logs(level)
        }
    }
}
impl Log for Logger {
    fn enabled(&self, metadata: &LogMetadata) -> bool {
        self.check_level(metadata.target(), metadata.level())
    }

    fn log(&self, record: &LogRecord) {
        if self.check_level(record.target(), record.level()) {
            let line = format!("[{}] [{}/{}] {}",
                               Local::now().format("%Y-%m-%d %H:%M:%S"),
                               record.target(), record.level(), record.args());
            if LOG_SENDER_EXISTS.load(Ordering::Acquire) {
                if let Err(_) = LOG_SENDER.with(|s| writeln!(s, "{}", line)) {
                    println!("{}", line);
                }
            } else {
                println!("{}", line);
            }
        }
    }
}

pub fn init() {
    set_logger(|max_log_level| {
        set_log_filter_obj(max_log_level);
        box Logger
    }).expect("failed to init logger!")
}