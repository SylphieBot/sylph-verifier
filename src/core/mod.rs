use commands::*;
use database::Database;
use errors::*;
use parking_lot::{Mutex, RwLock};
use std::mem::drop;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod config;
mod discord;
mod place;
mod roles;
mod statistics;
mod terminal;
mod verifier;

pub use self::config::{ConfigManager, ConfigKey, ConfigKeys};
pub use self::roles::{RoleManager, AssignedRole, ConfiguredRole};
pub use self::verifier::{Verifier, VerifyResult, TokenStatus, RekeyReason};

use self::discord::DiscordManager;
use self::place::PlaceManager;
use self::terminal::Terminal;

const STATUS_STOPPED : u8 = 0;
const STATUS_RUNNING : u8 = 1;
const STATUS_STOPPING: u8 = 2;

struct VerifierCoreData {
    status: AtomicU8,
    database: Database, config: ConfigManager, cmd_sender: CommandSender,
    terminal: Terminal, verifier: Verifier, discord: Mutex<DiscordManager>,
    place: PlaceManager, roles: RoleManager,
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
                             restarted.").ok();
            }
        }
    }
}

const PLACE_TARGET_NAME: &str = "Sylph-Verifier.rbxl";

#[derive(Clone)]
pub struct VerifierCore(Arc<VerifierCoreData>);
impl VerifierCore {
    pub fn new(root_path: PathBuf, database: Database) -> Result<VerifierCore> {
        let mut place_target = root_path.clone();
        place_target.push(PLACE_TARGET_NAME);

        let config = ConfigManager::new(database.clone());
        let cmd_sender = CommandSender::new();

        let terminal = Terminal::new(cmd_sender.clone())?;
        let verifier = Verifier::new(config.clone(), database.clone())?;
        let discord = Mutex::new(DiscordManager::new(config.clone(), cmd_sender.clone()));
        let place = PlaceManager::new(place_target)?;
        let roles = RoleManager::new(config.clone(), database.clone());

        Ok(VerifierCore(Arc::new(VerifierCoreData {
            status: AtomicU8::new(STATUS_STOPPED),
            database, config, cmd_sender, terminal, verifier, discord, place, roles,
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
                "VerifierCore already started.");
        let cmd_guard = self.0.cmd_sender.activate(&self.0);
        self.refresh_place()?;
        self.connect_discord()?;
        self.0.terminal.open()?;
        ensure!(self.0.status.load(Ordering::Relaxed) == STATUS_STOPPING,
                "Terminal interrupted without initializing shutdown!");
        drop(cmd_guard);
        self.0.discord.lock().shutdown()?;
        self.wait_on_instances();
        ensure!(self.0.status.compare_and_swap(STATUS_STOPPING, STATUS_STOPPED,
                                               Ordering::Relaxed) == STATUS_STOPPING,
                "VerifierCore not currently stopping?");
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
    pub fn roles(&self) -> &RoleManager {
        &self.0.roles
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

    pub fn refresh_place(&self) -> Result<()> {
        self.0.place.update_place(self)
    }
}

// This allows start() to safely take &self rather than self. This enforces a logical constraint,
// not a memory safety constraint.
impl !Sync for VerifierCore { }