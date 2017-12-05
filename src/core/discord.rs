use commands::*;
use core::*;
use errors::*;
use parking_lot::{Mutex, RwLock};
use serenity::Client;
use serenity::model::*;
use serenity::prelude::*;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;

const DEFAULT_PREFIX: &'static str = "!";

struct DiscordContext<'a> {
    ctx: Context, message: &'a Message, content: &'a str,
    privilege_level: PrivilegeLevel, command_target: CommandTarget,
}
impl <'a> CommandContextData for DiscordContext<'a> {
    fn privilege_level(&self) -> PrivilegeLevel {
        self.privilege_level
    }
    fn command_target(&self) -> CommandTarget {
        self.command_target
    }
    fn prefix(&self) -> &str {
        DEFAULT_PREFIX
    }
    fn message_content(&self) -> &str {
        self.content
    }
    fn respond(&self, message: &str, mention_user: bool) -> Result<()> {
        self.message.channel_id.send_message(|m| if mention_user {
            m.content(format_args!("<@{}> {}", self.message.author.id.0, message))
        } else {
            m.content(format_args!("{}", message))
        })?;
        Ok(())
    }
    fn discord_context(&self) -> Option<(&Context, &Message)> {
        Some((&self.ctx, self.message))
    }
}

struct Handler {
    core: Arc<RwLock<Option<VerifierCore>>>,
}
impl EventHandler for Handler {
    fn message(&self, ctx: Context, message: Message) {
        let command = if message.content.starts_with(DEFAULT_PREFIX) {
            let content = &message.content[DEFAULT_PREFIX.len()..];
            get_command(content)
        } else {
            None
        };

        if let Some(command) = command {
            info!("User '{}' used command: {}", message.author, message.content);
            if let Some(core) = self.core.read().as_ref().cloned() {
                core.catch_error(|| {
                    let content = &message.content[DEFAULT_PREFIX.len()..];
                    if let Some(ch) = message.channel() {
                        let (privilege_level, command_target) = match ch {
                            Channel::Guild(ch) => {
                                // TODO: Implement BotOwner
                                let guild = ch.read().guild().chain_err(|| "Guild not found.")?;
                                let owner_id = guild.read().owner_id;
                                (if message.author.id == owner_id {
                                    PrivilegeLevel::GuildOwner
                                } else {
                                    PrivilegeLevel::NormalUser
                                }, CommandTarget::ServerMessage)
                            }
                            Channel::Group(_) | Channel::Private(_) | Channel::Category(_) =>
                                (PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage),
                        };
                        let ctx = DiscordContext {
                            ctx, message: &message, content, privilege_level, command_target
                        };
                        command.run(&ctx, &core);
                    }
                    Ok(())
                }).ok();
            } else {
                message.channel_id.send_message(|m|
                    m.content(format_args!("<@{}> The verifier bot is currently shutting down \
                                            or restarting and cannot handle your command.",
                                           message.author.id.0))
                ).ok();
            }
        }

    }
}

const STATUS_NOT_INIT: u8 = 0;
const STATUS_RUNNING : u8 = 1;
const STATUS_SHUTDOWN: u8 = 2;

struct DiscordBot {
    token: String, status: AtomicU8,
    core: Arc<RwLock<Option<VerifierCore>>>, client: Arc<Mutex<Option<Client>>>,
}
impl DiscordBot {
    fn new(token: &str, core: Arc<RwLock<Option<VerifierCore>>>) -> Result<DiscordBot> {
        Ok(DiscordBot {
            core, client: Arc::new(Mutex::new(None)),
            token: token.to_string(), status: AtomicU8::new(STATUS_NOT_INIT),
        })
    }
    fn start(&self) -> Result<()> {
        ensure!(self.status.compare_and_swap(STATUS_NOT_INIT, STATUS_RUNNING,
                                             Ordering::Relaxed) == STATUS_NOT_INIT,
                "Discord component started twice!");
        let core = self.core.clone();
        let client = self.client.clone();
        *client.lock() = Some(Client::new(&self.token, Handler { core: core.clone() })?);
        thread::Builder::new().name("discord thread".to_string()).spawn(move || {
            core.read().as_ref().unwrap().catch_error(|| {
                let mut client = client.lock();
                match client.as_mut().unwrap().start_autosharded() {
                    Ok(_) | Err(SerenityError::Client(ClientError::Shutdown)) => { }
                    Err(err) => bail!(err),
                }
                Ok(())
            }).ok();
            info!("Shutting down Discord connection.");
            *client.lock() = None;
        })?;
        Ok(())
    }
    fn shutdown(&self) -> Result<()> {
        ensure!(self.status.compare_and_swap(STATUS_RUNNING, STATUS_SHUTDOWN,
                                             Ordering::Relaxed) == STATUS_RUNNING,
                "Already shutting down Discord component!");
        self.client.lock().as_ref().map(|x| x.shard_manager.lock().shutdown_all());
        Ok(())
    }
}

pub struct DiscordManager {
    core: Arc<RwLock<Option<VerifierCore>>>, bot: Option<DiscordBot>,
}
impl DiscordManager {
    pub fn new(core: &VerifierCore) -> DiscordManager {
        let core = Arc::new(RwLock::new(Some(core.clone())));
        DiscordManager { core, bot: None }
    }

    pub fn start(&mut self) -> Result<()> {
        if let &None = &self.bot {
            unimplemented!()
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        if let &Some(_) = &self.bot {
            self.bot.take().unwrap().shutdown()?;
        }
        Ok(())
    }
    pub fn restart(&mut self) -> Result<()> {
        self.stop()?;
        self.start()
    }
}
impl Drop for DiscordManager {
    fn drop(&mut self) {
        // Manually break the reference cycle, just in case.
        *self.core.write() = None
    }
}