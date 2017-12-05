use chrono::{Date, Local};
use errors::*;
use linefeed::reader::LogSender;
use log::*;
use parking_lot::Mutex;
use std::cmp::max;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

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
static LOG_LEVEL_LIB: AtomicLogLevel = AtomicLogLevel(AtomicU8::new(AtomicLogLevel::LOG_INFO));
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
const MODULE_PATH_INIT: &'static str = "sylph_verifier::";
fn is_app_source(source: &str) -> bool {
    source == "sylph_verifier" || source.starts_with(MODULE_PATH_INIT)
}
fn munge_target(target: &str) -> &str {
    if target.starts_with(MODULE_PATH_INIT) {
        &target[MODULE_PATH_INIT.len()..]
    } else {
        target
    }
}
fn check_level(source: &str, level: LogLevel) -> bool {
    if is_app_source(source) {
        LOG_LEVEL_APP.logs(level)
    } else {
        LOG_LEVEL_LIB.logs(level)
    }
}

static LOG_SENDER: Mutex<Option<LogSender>> = Mutex::new(None);
pub fn set_log_sender(sender: LogSender) {
    let mut cur_sender = LOG_SENDER.lock();
    if cur_sender.is_some() {
        panic!("Attempted to set log sender twice!")
    }
    *cur_sender = Some(sender);
}

enum LogFileOutput {
    NotInitialized, Initialized {
        out: BufWriter<File>, date: Date<Local>,
    }
}
impl LogFileOutput {
    fn refresh(&mut self, log_dir: &PathBuf) -> Result<()> {
        let mut out_path = log_dir.clone();
        let today = Local::today();
        out_path.push(format!("{}.log", today.format("%Y-%m-%d")));

        *self = LogFileOutput::Initialized {
            out: BufWriter::new(OpenOptions::new()
                .write(true).read(true).append(true).truncate(false).create(true)
                .open(out_path)?),
            date: Local::today(),
        };

        Ok(())
    }
    fn check_open_new(&mut self, log_dir: &PathBuf) -> Result<()> {
        let needs_refresh = match self {
            &mut LogFileOutput::NotInitialized => true,
            &mut LogFileOutput::Initialized { ref date, .. } => date != &Local::today(),
        };
        if needs_refresh {
            self.refresh(log_dir)?
        }
        Ok(())
    }
    fn log(&mut self, log_dir: &PathBuf, line: &str) -> Result<()> {
        self.check_open_new(log_dir)?;
        if let &mut LogFileOutput::Initialized { ref mut out, .. } = self {
            writeln!(out, "{}", line)?;
            out.flush()?;
            Ok(())
        } else {
            unreachable!()
        }
    }
}
static LOG_FILE: Mutex<LogFileOutput> = Mutex::new(LogFileOutput::NotInitialized);

struct Logger {
    log_dir: PathBuf,
}
fn log_raw(line: &str) {
    match LOG_SENDER.lock().as_ref() {
        Some(sender) => if let Err(_) = writeln!(sender, "{}", line) {
            println!("{}", line);
        }
        None => println!("{}", line),
    }
}
impl Log for Logger {
    fn enabled(&self, metadata: &LogMetadata) -> bool {
        check_level(metadata.target(), metadata.level())
    }

    fn log(&self, record: &LogRecord) {
        if check_level(record.target(), record.level()) {
            let now = Local::now().format("%Y-%m-%d %H:%M:%S");
            let line = if record.target() == "$raw" {
                format!("[{}] {}", now, record.args())
            } else {
                format!("[{}] [{}/{}] {}",
                        now, munge_target(record.target()), record.level(), record.args())
            };
            log_raw(&line);
            if let Err(_) = LOG_FILE.lock().log(&self.log_dir, &line) {
                log_raw(&format!("[{}] [{}/WARN] Failed to log line to disk!",
                                 now, munge_target(module_path!())))
            }
        }
    }
}

pub fn init<P: AsRef<Path>>(root_path: P) -> Result<()> {
    let mut log_dir = PathBuf::from(root_path.as_ref());
    log_dir.push("logs");
    fs::create_dir_all(&log_dir)?;

    LOG_FILE.lock().log(&log_dir, &format!("===== Starting logging at {} =====",
                                           Local::now().format("%Y-%m-%d %H:%M:%S")))?;
    set_logger(move |max_log_level| {
        set_log_filter_obj(max_log_level);
        box Logger { log_dir }
    }).expect("failed to init logger!");

    Ok(())
}