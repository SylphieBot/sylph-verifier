// TODO: Refactor, restructure, and clean up this module.

use error_report::*;
use errors::*;
use fs2::*;
use linefeed::reader::LogSender;
use parking_lot::*;
use roblox::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serenity::model::{UserId, GuildId};
use std::any::Any;
use std::fs::{File, OpenOptions};
use std::mem::drop;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[macro_use] mod database;

mod config;
mod discord;
mod terminal;
mod verifier;

pub use self::config::{ConfigKey, ConfigKeys};
pub use self::database::{DatabaseConnection, schema};
pub use self::verifier::{TokenStatus, RekeyReason};

use self::config::ConfigManager;
use self::database::Database;
use self::discord::DiscordManager;
use self::verifier::Verifier;

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
    _lock: File, database: Database, config: ConfigManager,
    verifier: Verifier, discord: Mutex<DiscordManager>,
}

#[derive(Clone)]
pub struct VerifierCore(Arc<VerifierCoreData>);
impl VerifierCore {
    pub fn new<P1: AsRef<Path>, P2: AsRef<Path>>(root_path: P1,
                                                 db_path: Option<P2>) -> Result<VerifierCore> {
        ensure!(!VERIFIER_CORE_CREATED.swap(true, Ordering::Relaxed),
                "Cannot create multiple VerifierCores.");

        let root_path = root_path.as_ref();
        let db_path = db_path.map_or_else(|| in_path(root_path, DB_FILE_NAME),
                                          |x| x.as_ref().into());

        let lock = match check_lock(in_path(root_path, LOCK_FILE_NAME)) {
            Ok(lock) => lock,
            Err(err) => {
                error!("Only one instance of Sylph-Verifier may be launched at once.");
                return Err(err)
            }
        };
        let database = Database::new(db_path)?;
        let verifier = Verifier::new(database.clone())?;
        let core = VerifierCore(Arc::new(VerifierCoreData {
            root_path: root_path.to_owned(), shutdown_sender: Mutex::new(None),
            status: AtomicU8::new(STATUS_NOT_INIT),
            _lock: lock, database, config: ConfigManager::new(),
            verifier, discord: Mutex::new(DiscordManager::new()),
        }));
        core.0.verifier.check_update(&core)?;
        core.0.discord.lock().set_core(&core);
        Ok(core)
    }
    pub fn start(self) -> Result<()> {
        // TODO: Do better error handling in this.
        ensure!(self.0.status.compare_and_swap(STATUS_NOT_INIT, STATUS_STARTING,
                                               Ordering::Relaxed) == STATUS_NOT_INIT,
                "VerifierCore already running.");
        self.connect_discord()?;
        let mut terminal = terminal::Terminal::new(&self)?;
        *self.0.shutdown_sender.lock() = Some(terminal.new_sender());
        ensure!(self.0.status.compare_and_swap(STATUS_STARTING, STATUS_RUNNING,
                                               Ordering::Relaxed) == STATUS_STARTING,
                "VerifierCore status corrupted: expected STATUS_STARTING");
        terminal.start()?;
        ensure!(self.0.status.load(Ordering::Relaxed) == STATUS_SHUTDOWN,
                "Terminal interrupted without initializing shutdown!");
        self.0.discord.lock().shutdown()?;
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

    pub fn get_verified_roblox_user(&self, user: UserId) -> Result<Option<RobloxUserID>> {
        self.0.verifier.get_verified_roblox_user(user)
    }
    pub fn get_verified_discord_user(&self, user: RobloxUserID) -> Result<Option<UserId>> {
        self.0.verifier.get_verified_discord_user(user)
    }
    pub fn try_verify(
        &self, discord_id: UserId, roblox_id: RobloxUserID, token: &str
    ) -> Result<TokenStatus> {
        self.0.verifier.try_verify(discord_id, roblox_id, token)
    }
    pub fn rekey(&self, force: bool) -> Result<()> {
        if force {
            self.0.verifier.rekey(self)
        } else {
            self.0.verifier.check_update(self)
        }
    }

    pub fn set_config<T: Serialize + DeserializeOwned + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>, value: T
    ) -> Result<()> {
        self.0.config.set(self, &self.0.database.connect()?, guild, key, value)
    }
    pub fn reset_config<T: Serialize + DeserializeOwned + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<()> {
        self.0.config.reset(self, &self.0.database.connect()?, guild, key)
    }
    pub fn get_config<T: Serialize + DeserializeOwned + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        self.0.config.get(&self.0.database.connect()?, guild, key)
    }

    pub fn connect_discord(&self) -> Result<()> {
        self.0.discord.lock().connect()
    }
    pub fn disconnect_discord(&self) -> Result<()> {
        self.0.discord.lock().disconnect()
    }
    pub fn reconnect_discord(&self) -> Result<()> {
        self.0.discord.lock().reconnect()
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
        self.0.verifier.add_config(&mut config);
        config
    }
}
