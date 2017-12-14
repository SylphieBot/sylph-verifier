use commands::*;
use errors::*;
use parking_lot::*;
use roblox::*;
use std::mem::drop;
use std::path::Path;
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
pub use self::verifier::{Verifier, TokenStatus, RekeyReason};

use self::database::Database;
use self::discord::DiscordManager;
use self::terminal::Terminal;

const STATUS_STOPPED : u8 = 0;
const STATUS_RUNNING : u8 = 1;
const STATUS_STOPPING: u8 = 2;

struct VerifierCoreData {
    status: AtomicU8,
    database: Database, config: ConfigManager, cmd_sender: CommandSender,
    terminal: Terminal, verifier: Verifier, discord: Mutex<DiscordManager>,
}

struct CommandSenderActiveGuard<'a>(&'a CommandSender);
impl <'a> Drop for CommandSenderActiveGuard<'a> {
    fn drop(&mut self) {
        *(self.0).0.write() = None
    }
}

#[derive(Clone)]
struct CommandSender(Arc<RwLock<Option<Arc<VerifierCoreData>>>>);
impl CommandSender {
    fn new() -> CommandSender {
        CommandSender(Arc::new(RwLock::new(None)))
    }
    fn activate(&self, core: &Arc<VerifierCoreData>) -> CommandSenderActiveGuard {
        *self.0.write() = Some(core.clone());
        CommandSenderActiveGuard(self)
    }

    pub fn is_alive(&self) -> bool {
        if let Some(core) = self.0.read().as_ref() {
            core.status.load(Ordering::Relaxed) == STATUS_RUNNING
        } else {
            false
        }
    }
    pub fn run_command(&self, command: &Command, ctx: &CommandContextData) {
        let core = self.0.read().as_ref().map(|x| VerifierCore(x.clone()));
        match core {
            Some(core) => command.run(ctx, &core),
            None => {
                ctx.respond("The bot is currently shutting down. Please wait until it is \
                             restarted.", true).ok();
            }
        }
    }
}

#[derive(Clone)]
pub struct VerifierCore(Arc<VerifierCoreData>);
impl VerifierCore {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<VerifierCore> {
        let database = Database::new(db_path.as_ref())?;
        let config = ConfigManager::new(database.clone());
        let cmd_sender = CommandSender::new();

        let terminal = Terminal::new(cmd_sender.clone())?;
        let verifier = Verifier::new(config.clone(), database.clone())?;
        let discord = Mutex::new(DiscordManager::new(config.clone(), cmd_sender.clone()));

        Ok(VerifierCore(Arc::new(VerifierCoreData {
            status: AtomicU8::new(STATUS_STOPPED),
            database, config, cmd_sender, terminal, verifier, discord,
        })))
    }
    fn wait_on_instances(&self) {
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
    }
    pub fn start(&self) -> Result<()> {
        ensure!(self.0.status.compare_and_swap(STATUS_STOPPED, STATUS_RUNNING,
                                               Ordering::Relaxed) == STATUS_STOPPED,
                "VerifierCore already running.");
        let cmd_guard = self.0.cmd_sender.activate(&self.0);
        self.connect_discord()?;
        self.0.terminal.open()?;
        ensure!(self.0.status.load(Ordering::Relaxed) == STATUS_STOPPING,
                "Terminal interrupted without initializing shutdown!");
        drop(cmd_guard);
        self.0.discord.lock().shutdown()?;
        self.wait_on_instances();
        ensure!(self.0.status.compare_and_swap(STATUS_STOPPING, STATUS_STOPPED,
                                               Ordering::Relaxed) == STATUS_STOPPING,
                "VerifierCore not currently stopping??");
        Ok(())
    }
    pub fn shutdown(&self) -> Result<()> {
        match self.0.status.compare_and_swap(STATUS_RUNNING, STATUS_STOPPING, Ordering::Relaxed) {
            STATUS_STOPPED  => bail!("VerifierCore not started yet."),
            STATUS_RUNNING  => {
                self.0.terminal.interrupt();
                Ok(())
            },
            STATUS_STOPPING => bail!("VerifierCore already shutting down."),
            _               => unreachable!(),
        }
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

    pub fn place_config(&self) -> Result<Vec<LuaConfigEntry>> {
        let mut config = Vec::new();
        // TODO: Make these dynamic from configuration.
        config.push(LuaConfigEntry::new("title", false, "Roblox Account Verifier"));
        config.push(LuaConfigEntry::new("intro_text", false, "\
            To verify your Roblox account on <Discord Server Name>, please enter the following \
            command in the #<channel name> channel.\
        "));
        config.push(LuaConfigEntry::new("bot_prefix", false,
                                        self.config().get(None, ConfigKeys::CommandPrefix)?));
        config.push(LuaConfigEntry::new("background_image", false, None as Option<&str>));
        self.0.verifier.add_config(&mut config);
        Ok(config)
    }
}

// This allows start() to safely take &self rather than self.
impl !Sync for VerifierCore { }