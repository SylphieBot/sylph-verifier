use chrono::{Date, Local};
use errors::*;
use log::*;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs;
use std::fs::{File, OpenOptions};
use std::fmt::{Write as FmtWrite};
use std::io::{BufWriter, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

// TODO: Use log4rs, which provides much of the functionality of this module.

#[derive(Debug)]
struct LogPolicy {
    module: &'static str, console: LevelFilter, log: LevelFilter,
}
impl LogPolicy {
    const fn new(module: &'static str, console: LevelFilter, log: LevelFilter) -> LogPolicy {
        LogPolicy { module, console, log }
    }
}
static LOG_POLICY: &'static [LogPolicy] = &[
    LogPolicy::new("sylph_verifier", LevelFilter::Info, LevelFilter::Trace),
    LogPolicy::new("hyper"         , LevelFilter::Info, LevelFilter::Info),
    LogPolicy::new("tokio_core"    , LevelFilter::Info, LevelFilter::Info),
    LogPolicy::new("tokio_reactor" , LevelFilter::Info, LevelFilter::Info),
    LogPolicy::new("*"             , LevelFilter::Info, LevelFilter::Debug),
];

fn is_in_module(module: &str, path: &str) -> bool {
    module == "*" || module == path || (
        path.len() > module.len() + 2 &&
            path.starts_with(module) && &path[module.len()..module.len()+2] == "::"
    )
}
fn source_info(source: &str) -> &'static LogPolicy {
    for level in LOG_POLICY {
        if is_in_module(level.module, source) {
            return level
        }
    }
    unreachable!() // due to the "*" entry
}
fn logs(filter: LevelFilter, level: Level) -> bool {
    match filter.to_level() {
        None => false,
        Some(filter) => filter >= level,
    }
}

const MODULE_PATH_INIT: &str = "sylph_verifier::";
fn munge_target(target: &str) -> &str {
    if target.starts_with(MODULE_PATH_INIT) {
        &target[MODULE_PATH_INIT.len()..]
    } else {
        target
    }
}

static LOG_SENDER_LOCKED: AtomicBool = AtomicBool::new(false);
static LOG_SENDER: Mutex<Option<Box<dyn Fn(&str) -> Result<()> + Send + Sync>>> = Mutex::new(None);
pub fn set_log_sender(sender: impl Fn(&str) -> Result<()> + Send + Sync + 'static) {
    if !LOG_SENDER_LOCKED.load(Ordering::Relaxed) {
        *LOG_SENDER.lock() = Some(Box::new(sender));
    }
}
pub fn lock_log_sender() {
    if !LOG_SENDER_LOCKED.compare_and_swap(false, true, Ordering::Relaxed) {
        *LOG_SENDER.lock() = None;
    }
}
pub fn remove_log_sender() {
    if !LOG_SENDER_LOCKED.load(Ordering::Relaxed) {
        *LOG_SENDER.lock() = None;
    }
}

#[cfg(windows)]
const NEW_LINE: &str = "\r\n"; // Because Notepad exists.

#[cfg(not(windows))]
const NEW_LINE: &str = "\n";

enum LogFileOutput {
    NotInitialized,
    Initialized { out: BufWriter<File>, date: Date<Local> }
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
            LogFileOutput::NotInitialized => true,
            LogFileOutput::Initialized { ref date, .. } => date != &Local::today(),
        };
        if needs_refresh {
            self.refresh(log_dir)?
        }
        Ok(())
    }
    fn log(&mut self, log_dir: &PathBuf, line: &str) -> Result<()> {
        self.check_open_new(log_dir)?;
        if let LogFileOutput::Initialized { ref mut out, .. } = self {
            write!(out, "{}{}", line, NEW_LINE)?;
            out.flush()?;
            Ok(())
        } else {
            unreachable!()
        }
    }
}
static LOG_FILE: Mutex<LogFileOutput> = Mutex::new(LogFileOutput::NotInitialized);

const STORE_LOG_LINES: usize = 40;
enum ErrorLogBuf {
    NotInitialized,
    Initialized(VecDeque<String>),
}
impl ErrorLogBuf {
    fn push_log(&mut self, line: String) {
        if let ErrorLogBuf::NotInitialized = self {
            *self = ErrorLogBuf::Initialized(VecDeque::new());
        }
        if let ErrorLogBuf::Initialized(ref mut vec) = self {
            vec.push_back(line);
            if vec.len() > STORE_LOG_LINES {
                vec.pop_front();
            }
        } else {
            unreachable!()
        }
    }

    fn format_logs(&self) -> Result<String> {
        if let ErrorLogBuf::Initialized(ref vec) = self {
            let mut buffer = String::new();
            for line in vec {
                writeln!(buffer, "{}", line)?;
            }
            Ok(buffer)
        } else {
            Ok("no logs found".to_owned())
        }
    }
}
static ERROR_LOG_BUF: Mutex<ErrorLogBuf> = Mutex::new(ErrorLogBuf::NotInitialized);

struct Logger {
    log_dir: PathBuf,
}
fn log_raw(line: &str) {
    match LOG_SENDER.lock().as_ref() {
        Some(sender) => if sender(line).is_err() {
            println!("{}", line);
        }
        None => println!("{}", line),
    }
}
impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let info = source_info(metadata.target());
        let level = metadata.level();

        logs(info.console, level) || logs(info.log, level)
    }

    fn log(&self, record: &Record) {
        let info = source_info(record.target());
        let level = record.level();
        let log_console = logs(info.console, level) && record.target() != "$command_input";
        let log_file = logs(info.log, level);

        if log_console || log_file {
            let now = Local::now().format("%Y-%m-%d %H:%M:%S");
            let line = if record.target() == "$raw" {
                format!("[{}] {}", now, record.args())
            } else if record.target() == "$command_input" {
                format!("sylph-verifier> {}", record.args())
            } else {
                format!("[{}] [{}/{}] {}",
                        now, munge_target(record.target()), record.level(), record.args())
            };

            if log_console {
                log_raw(&line);
            }
            if log_file {
                if LOG_FILE.lock().log(&self.log_dir, &line).is_err() {
                    log_raw(&format!("[{}] [{}/WARN] Failed to log line to disk!",
                                     now, munge_target(module_path!())))
                }
                ERROR_LOG_BUF.lock().push_log(line);
            }
        }
    }

    fn flush(&self) {
        // Not used
    }
}

pub fn format_recent_logs() -> Result<String> {
    ERROR_LOG_BUF.lock().format_logs()
}

pub fn init(root_path: impl AsRef<Path>) -> Result<()> {
    let mut log_dir = PathBuf::from(root_path.as_ref());
    log_dir.push("logs");
    fs::create_dir_all(&log_dir)?;

    LOG_FILE.lock().log(&log_dir, &format!("===== Starting logging at {} =====",
                                           Local::now().format("%Y-%m-%d %H:%M:%S")))?;
    set_max_level(LevelFilter::Trace);
    set_boxed_logger(Box::new(Logger { log_dir })).expect("failed to init logger!");

    Ok(())
}