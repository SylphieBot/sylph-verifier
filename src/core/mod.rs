use errors::*;
use fs2::*;
use linefeed::reader::LogSender;
use log::LogLevelFilter;
use parking_lot::*;
use roblox::*;
use std::fs::{File, OpenOptions};
use std::panic::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod database;
mod discord;
mod error_report;
mod logger;
mod terminal;
mod token;

pub use self::database::{DatabaseConnection, schema};
pub use self::token::{TokenStatus, RekeyReason};

use self::database::Database;
use self::token::TokenContext;

const LOCK_FILE_NAME: &'static str = "Sylph-Verifier.lock";
const DB_FILE_NAME: &'static str = "Sylph-Verifier.db";

fn check_lock<P: AsRef<Path>>(path: P) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    let lock_file = options.open(path)?;
    lock_file.try_lock_exclusive()?;
    Ok(lock_file)
}
fn in_path<P: AsRef<Path>>(root_path: P, file: &str) -> PathBuf {
    let mut path = PathBuf::new();
    path.push(root_path);
    path.push(file);
    path
}

static VERIFIER_CORE_CREATED: AtomicBool = AtomicBool::new(false);

const STATUS_NOT_INIT: u8 = 0;
const STATUS_STARTING: u8 = 1;
const STATUS_RUNNING : u8 = 2;
const STATUS_SHUTDOWN: u8 = 3;
const STATUS_UNINIT  : u8 = 4;

struct VerifierCoreData {
    root_path: PathBuf, shutdown_sender: Mutex<Option<LogSender>>, status: AtomicU8,
    _lock: File, database: database::Database, token_context: RwLock<TokenContext>,
}

#[derive(Clone)]
pub struct VerifierCore(Arc<VerifierCoreData>);
impl VerifierCore {
    pub fn new<P1: AsRef<Path>, P2: AsRef<Path>>(root_path: P1,
                                                 db_path: Option<P2>) -> Result<VerifierCore> {
        ensure!(!VERIFIER_CORE_CREATED.swap(true, Ordering::Relaxed),
                "Cannot create multiple VerifierCores.");

        let root_path = root_path.as_ref();
        logger::init(root_path)?;
        error_report::init_panic_hook();
        let db_path = db_path.map_or_else(|| in_path(root_path, DB_FILE_NAME),
                                          |x| x.as_ref().into());
        error_report::report_error(root_path, error_report::catch_panic(root_path, move || {
            let lock = match check_lock(in_path(root_path, LOCK_FILE_NAME)) {
                Ok(lock) => lock,
                Err(err) => {
                    error!("Only one instance of Sylph-Verifier may be launched at once.");
                    return Err(err)
                }
            };
            let database = Database::new(db_path)?;
            let token_context = RwLock::new(TokenContext::from_db(&database.connect()?, 300)?);
            Ok(VerifierCore(Arc::new(VerifierCoreData {
                root_path: root_path.to_owned(), shutdown_sender: Mutex::new(None),
                status: AtomicU8::new(STATUS_NOT_INIT),
                _lock: lock, database, token_context
            })))
        })?)
    }
    pub fn start(self) -> Result<()> {
        ensure!(self.0.status.compare_and_swap(STATUS_NOT_INIT, STATUS_STARTING,
                                               Ordering::Relaxed) == STATUS_NOT_INIT,
                "VerifierCore already running.");
        let mut terminal = terminal::Terminal::new(&self)?;
        *self.0.shutdown_sender.lock() = Some(terminal.new_sender());
        ensure!(self.0.status.compare_and_swap(STATUS_STARTING, STATUS_RUNNING,
                                               Ordering::Relaxed) == STATUS_STARTING,
                "VerifierCore status corrupted: expected STATUS_STARTING");
        terminal.start()?;
        ensure!(self.0.status.load(Ordering::Relaxed) == STATUS_SHUTDOWN,
                "Terminal interrupted without initializing shutdown!");
        let mut next_message = Instant::now() + Duration::from_secs(1);
        let mut printed_waiting = false;
        loop {
            let count = Arc::strong_count(&self.0);
            if count == 1 { break }
            if Instant::now() > next_message {
                info!("Waiting on {} threads to stop. Press {}+C to force shutdown.", count - 1,
                      if env!("TARGET").contains("apple-darwin") { "Command" } else { "Ctrl" });
                next_message = Instant::now() + Duration::from_secs(5);
                printed_waiting = true;
            }
            thread::yield_now()
        }
        if printed_waiting {
            info!("All threads stopped. Shutting down.")
        }
        ensure!(self.0.status.compare_and_swap(STATUS_SHUTDOWN, STATUS_UNINIT,
                                               Ordering::Relaxed) == STATUS_SHUTDOWN,
                "VerifierCore status corrupted: expected STATUS_SHUTDOWN");
        Ok(())
    }
    pub fn shutdown(&self) -> Result<()> {
        match self.0.status.compare_and_swap(STATUS_RUNNING, STATUS_SHUTDOWN, Ordering::Relaxed) {
            STATUS_NOT_INIT => bail!("VerifierCore not started yet."),
            STATUS_STARTING => bail!("VerifierCore not fully started yet."),
            STATUS_RUNNING  => {
                self.0.shutdown_sender.lock().as_ref().unwrap().interrupt().ok();
                Ok(())
            },
            STATUS_SHUTDOWN => bail!("VerifierCore already shutting down."),
            STATUS_UNINIT   => bail!("VerifierCore already shut down."),
            _               => unreachable!(),
        }
    }
    pub fn is_alive(&self) -> bool {
        self.0.status.load(Ordering::Relaxed) == STATUS_RUNNING
    }

    pub fn check_token(&self, user: RobloxUserID, token: &str) -> Result<TokenStatus> {
        self.0.token_context.read().check_token(user, token)
    }
    pub fn rekey(&self) -> Result<()> {
        let db = self.0.database.connect()?;
        db.transaction(|| {
            let mut token_context = self.0.token_context.write();
            *token_context = TokenContext::rekey(&db, 300)?;
            Ok(())
        })
    }

    pub fn set_app_log_level(&self, filter: LogLevelFilter) {
        logger::set_app_filter_level(filter)
    }
    pub fn set_lib_log_level(&self, filter: LogLevelFilter) {
        logger::set_lib_filter_level(filter)
    }

    pub fn place_config(&self) -> Vec<LuaConfigEntry> {
        let mut config = Vec::new();
        // TODO: Make these dynamic from configuration.
        config.push(LuaConfigEntry::new("title", false, "Roblox Account Verifier"));
        config.push(LuaConfigEntry::new("intro_text", false, "\
            To verify your Roblox account on <Discord Server Name>, please enter the following \
            command in the #<channel name> channel.\
        "));
        config.push(LuaConfigEntry::new("bot_prefix", false, "!"));
        config.push(LuaConfigEntry::new("background_image", false, None as Option<&str>));
        self.0.token_context.read().add_config(&mut config);
        config
    }

    pub fn catch_panic<F, R>(&self, f: F) -> Result<R> where F: FnOnce() -> R {
        error_report::catch_panic(&self.0.root_path, AssertUnwindSafe(f))
    }
    pub fn catch_error<F, T>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.catch_panic(||
            error_report::report_error(&self.0.root_path, f())
        )?
    }
}

// Utility function
pub trait UnwrapReport<T> {
    fn unwrap_report(self) -> T;
}
impl <T> UnwrapReport<T> for Result<T> {
    fn unwrap_report(self) -> T {
        error_report::unwrap_fatal(self)
    }
}