use errors::*;
use fs2::*;
use parking_lot::*;
use roblox::*;
use std::fs::{File, OpenOptions};
use std::panic::*;
use std::path::{Path, PathBuf};

mod database;
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

pub struct VerifierCore {
    root_path: PathBuf,
    _lock: File, database: database::Database, token_context: RwLock<TokenContext>,
}
impl VerifierCore {
    pub fn new<P1: AsRef<Path>, P2: AsRef<Path>>(root_path: P1,
                                                 db_path: Option<P2>) -> Result<VerifierCore> {
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
            Ok(VerifierCore {
                root_path: root_path.to_owned(),
                _lock: lock, database, token_context
            })
        })?)
    }

    pub fn check_token(&self, user: RobloxUserID, token: &str) -> Result<TokenStatus> {
        self.token_context.read().check_token(user, token)
    }
    pub fn rekey(&self) -> Result<()> {
        let mut token_context = self.token_context.write();
        *token_context = TokenContext::rekey(&self.database.connect()?, 300)?;
        Ok(())
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
        self.token_context.read().add_config(&mut config);
        config
    }

    pub fn run_terminal(&self) -> Result<!> {
        // TODO: Do some tracking on this
        terminal::init(&self.database)
    }

    pub fn catch_panic<F, R>(&self, f: F) -> Result<R> where F: FnOnce() -> R + UnwindSafe {
        error_report::catch_panic(&self.root_path, f)
    }
    pub fn catch_error<F, T>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        error_report::report_error(&self.root_path, f())
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

// Assert VerifierCore is Sync
fn _check_sync<T: Sync>(_: T) { }
fn _is_verifier_core_sync() {
    let core: VerifierCore = unsafe { ::std::mem::uninitialized() };
    _check_sync(core)
}