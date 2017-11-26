use errors::*;
use fs2::*;
use parking_lot::*;
use roblox::*;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

mod database;
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
    lock: File, database: database::Database, token_context: RwLock<TokenContext>,
}
impl VerifierCore {
    pub fn new<P1: AsRef<Path>, P2: AsRef<Path>>(root_path: P1,
                                                 db_path: Option<P2>) -> Result<VerifierCore> {
        logger::init();
        let lock = match check_lock(in_path(root_path.as_ref(), LOCK_FILE_NAME)) {
            Ok(lock) => lock,
            Err(err) => {
                error!("Only one instance of Sylph-Verifier may be launched at once.");
                return Err(err)
            }
        };
        let db_path = db_path.map_or_else(|| in_path(root_path.as_ref(), DB_FILE_NAME),
                                          |x| x.as_ref().into());
        let database = Database::new(db_path)?;
        let token_context = RwLock::new(TokenContext::from_db(&database.connect()?, 300)?);
        Ok(VerifierCore { lock, database, token_context })
    }

    pub fn check_token(&self, user: RobloxUserID, token: &str) -> Result<TokenStatus> {
        self.token_context.read().check_token(user, token)
    }
    pub fn rekey(&self) -> Result<()> {
        let mut token_context = self.token_context.write();
        *token_context = TokenContext::new_in_db(&self.database.connect()?, 300)?;
        Ok(())
    }

    pub fn place_config(&self) -> Vec<LuaConfigEntry> {
        let mut config = Vec::new();
        self.token_context.read().add_config(&mut config);
        config
    }

    pub fn run_terminal(&self) -> Result<!> {
        // TODO: Do some tracking on this
        terminal::init(&self.database)
    }
}

// Assert VerifierCore is Sync
fn _check_sync<T: Sync>(t: T) { }
fn _is_verifier_core_sync() {
    let core: VerifierCore = unsafe { ::std::mem::uninitialized() };
    _check_sync(core)
}