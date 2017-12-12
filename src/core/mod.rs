// TODO: Refactor, restructure, and clean up this module.

use commands::*;
use errors::*;
use fs2::*;
use linefeed::reader::LogSender;
use parking_lot::*;
use roblox::*;
use serenity::model::UserId;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[macro_use] mod database;

mod config;
mod discord;
mod terminal;
mod verifier;

pub use self::config::{ConfigManager, ConfigKey, ConfigKeys};
pub use self::database::{DatabaseConnection, schema};
pub use self::verifier::{TokenStatus, RekeyReason};

use self::database::Database;
use self::discord::DiscordManager;
use self::terminal::Terminal;
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

#[derive(Clone)]
struct CommandSender(Arc<RwLock<Option<VerifierCore>>>);
impl CommandSender {
    fn new() -> CommandSender {
        CommandSender(Arc::new(RwLock::new(None)))
    }
    fn init(&self, core: &VerifierCore) {
        *self.0.write() = Some(core.clone())
    }
    pub fn run_command(&self, command: &Command, ctx: &CommandContextData) {
        let core = self.0.read().as_ref().cloned();
        match core {
            Some(core) => command.run(ctx, &core),
            None => {
                ctx.respond("The bot is currently shutting down. Please wait until it is \
                                 restarted.", true).ok();
            }
        }
    }
    pub fn is_alive(&self) -> bool {
        if let Some(core) = self.0.read().as_ref() {
            core.is_alive()
        } else {
            false
        }
    }
    fn shutdown(&self) {
        *self.0.write() = None
    }
}

const STATUS_NOT_INIT: u8 = 0;
const STATUS_STARTING: u8 = 1;
const STATUS_RUNNING : u8 = 2;
const STATUS_SHUTDOWN: u8 = 3;
const STATUS_UNINIT  : u8 = 4;

struct VerifierCoreData {
    status: AtomicU8,
    _lock: File, database: Database, config: ConfigManager, cmd_sender: CommandSender,
    terminal: Terminal, verifier: Verifier, discord: Mutex<DiscordManager>,
}

#[derive(Clone)]
pub struct VerifierCore(Arc<VerifierCoreData>);
impl VerifierCore {
    pub fn new<P1: AsRef<Path>, P2: AsRef<Path>>(root_path: P1,
                                                 db_path: Option<P2>) -> Result<VerifierCore> {
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
        let config = ConfigManager::new(database.clone());
        let cmd_sender = CommandSender::new();

        let terminal = Terminal::new(cmd_sender.clone())?;
        let verifier = Verifier::new(config.clone(), database.clone())?;
        let discord = Mutex::new(DiscordManager::new(config.clone(), cmd_sender.clone()));

        let core = VerifierCore(Arc::new(VerifierCoreData {
            status: AtomicU8::new(STATUS_NOT_INIT),
            _lock: lock, database, config, cmd_sender,
            terminal, verifier, discord,
        }));
        core.0.cmd_sender.init(&core);
        Ok(core)
    }
    pub fn start(self) -> Result<()> {
        // TODO: Do better error handling in this.
        ensure!(self.0.status.compare_and_swap(STATUS_NOT_INIT, STATUS_STARTING,
                                               Ordering::Relaxed) == STATUS_NOT_INIT,
                "VerifierCore already running.");
        self.connect_discord()?;
        ensure!(self.0.status.compare_and_swap(STATUS_STARTING, STATUS_RUNNING,
                                               Ordering::Relaxed) == STATUS_STARTING,
                "VerifierCore status corrupted: expected STATUS_STARTING");
        self.0.terminal.start()?;
        ensure!(self.0.status.load(Ordering::Relaxed) == STATUS_SHUTDOWN,
                "Terminal interrupted without initializing shutdown!");
        self.0.cmd_sender.shutdown();
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
                self.0.terminal.interrupt();
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

    pub fn config(&self) -> &ConfigManager {
        &self.0.config
    }
    pub fn verifier(&self) -> &Verifier {
        &self.0.verifier
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
