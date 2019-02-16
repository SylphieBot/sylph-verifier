use commands::*;
use database::Database;
use errors::*;
use parking_lot::RwLock;
use std::mem::drop;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod config;
mod delete_service;
mod discord;
mod place;
mod roles;
mod tasks;
mod terminal;
mod verification_channel;
mod verifier;

pub use self::config::{ConfigManager, ConfigKey, ConfigKeys};
pub use self::roles::{RoleManager, AssignedRole, ConfiguredRole, SetRolesStatus};
pub use self::verification_channel::VerificationChannelManager;
pub use self::verifier::{Verifier, VerifyResult, TokenStatus};

use self::delete_service::DeleteService;
use self::discord::DiscordManager;
use self::place::PlaceManager;
use self::terminal::Terminal;
use self::tasks::TaskManager;

const STATUS_STOPPED : u8 = 0;
const STATUS_RUNNING : u8 = 1;
const STATUS_STOPPING: u8 = 2;

struct VerifierCoreData {
    status: AtomicU8,
    _database: Database, config: ConfigManager, core_ref: CoreRef,
    terminal: Terminal, verifier: Verifier, discord: DiscordManager,
    place: PlaceManager, roles: RoleManager, _tasks: TaskManager,
    verify_channel: VerificationChannelManager,
}

struct CoreRefActiveGuard<'a>(&'a CoreRef);
impl <'a> Drop for CoreRefActiveGuard<'a> {
    fn drop(&mut self) {
        *(self.0).0.write() = None
    }
}

#[derive(Clone)]
struct CoreRef(Arc<RwLock<Option<Arc<VerifierCoreData>>>>);
impl CoreRef {
    fn new() -> CoreRef {
        CoreRef(Arc::new(RwLock::new(None)))
    }
    fn activate(&self, core: &Arc<VerifierCoreData>) -> CoreRefActiveGuard {
        *self.0.write() = Some(core.clone());
        CoreRefActiveGuard(self)
    }

    fn is_alive(&self) -> bool {
        if let Some(core) = self.0.read().as_ref() {
            core.status.load(Ordering::Relaxed) == STATUS_RUNNING
        } else {
            false
        }
    }
    fn get_core(&self) -> Option<VerifierCore> {
        self.0.read().as_ref().map(|x| VerifierCore(x.clone()))
    }
    fn run_command(&self, command: &Command, ctx: &dyn CommandContextData) {
        if let Some(core) = self.get_core() {
            command.run(ctx, &core);
        } else  {
            ctx.respond("The bot is currently shutting down. Please wait until it is \
                         restarted.").ok();
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
        let core_ref = CoreRef::new();

        let tasks = TaskManager::new(core_ref.clone())?;
        let delete_service = DeleteService::new(tasks.clone());
        let terminal = Terminal::new(core_ref.clone())?;
        let verify_channel = VerificationChannelManager::new(config.clone(), database.clone(),
                                                             delete_service.clone());
        let verifier = Verifier::new(config.clone(), database.clone())?;
        let place = PlaceManager::new(place_target)?;
        let roles = RoleManager::new(config.clone(), database.clone(), verifier.clone(),
                                     tasks.clone());
        let discord = DiscordManager::new(config.clone(), core_ref.clone(), roles.clone(),
                                          tasks.clone(), verify_channel.clone(),
                                          delete_service.clone());

        tasks.dispatch_repeating_task(Duration::from_secs(60 * 10), |core| core.cleanup());

        Ok(VerifierCore(Arc::new(VerifierCoreData {
            status: AtomicU8::new(STATUS_STOPPED),
            _database: database, _tasks: tasks,
            config, core_ref, terminal, verifier, discord, place, roles, verify_channel,
        })))
    }

    fn cleanup(&self) -> Result<()> {
        debug!("Running garbage collection.");
        self.0.config.on_cleanup_tick();
        self.0.discord.on_cleanup_tick();
        self.0.roles.on_cleanup_tick();
        self.0.verify_channel.on_cleanup_tick();
        self.0.verifier.on_cleanup_tick();
        Ok(())
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
    pub fn start(self) -> Result<()> {
        ensure!(self.0.status.compare_and_swap(STATUS_STOPPED, STATUS_RUNNING,
                                               Ordering::Relaxed) == STATUS_STOPPED,
                "VerifierCore already started.");
        let core_ref_guard = self.0.core_ref.activate(&self.0);
        self.refresh_place()?;
        self.0.discord.connect()?;
        self.0.terminal.open()?;
        ensure!(self.0.status.load(Ordering::Relaxed) == STATUS_STOPPING,
                "Terminal interrupted without initializing shutdown!");
        drop(core_ref_guard);
        self.0.discord.shutdown()?;
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
    pub fn discord(&self) -> &DiscordManager {
        &self.0.discord
    }
    pub fn verify_channel(&self) -> &VerificationChannelManager {
        &self.0.verify_channel
    }

    pub fn refresh_place(&self) -> Result<()> {
        self.0.place.update_place(self)
    }
}