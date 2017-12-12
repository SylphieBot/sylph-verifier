use commands::*;
use core::*;
use core::database::*;
use error_report;
use parking_lot::{Mutex, RwLock};
use serenity::Client;
use serenity::client::bridge::gateway::ShardManager;
use serenity::model::*;
use serenity::prelude::*;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;

struct DiscordContext<'a> {
    ctx: Context, message: &'a Message, content: &'a str, prefix: String,
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
        &self.prefix
    }
    fn message_content(&self) -> &str {
        self.content
    }
    fn respond(&self, message: &str, mention_user: bool) -> Result<()> {
        self.message.channel_id.send_message(|m| if mention_user {
            if message.contains("\n") {
                m.content(format_args!("<@{}>\n{}", self.message.author.id, message))
            } else {
                m.content(format_args!("<@{}> {}", self.message.author.id, message))
            }
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
    core: Arc<RwLock<Option<VerifierCore>>>, user_prefix: RwLock<Option<String>>,
}
impl EventHandler for Handler {
    fn ready(&self, _: Context, ready: Ready) {
        *self.user_prefix.write() = Some(format!("<@{}> ", ready.user.id))
    }

    fn message(&self, ctx: Context, message: Message) {
        let core = match self.core.read().as_ref().cloned() {
            Some(core) => core,
            None => return,
        };
        let prefix = error_report::catch_error(||
            core.config().get(message.guild_id(), ConfigKeys::CommandPrefix)
        );
        let prefix = match prefix {
            Ok(prefix) => prefix,
            Err(_) => return,
        };

        let content = if message.content.starts_with(&prefix) {
            Some(&message.content[prefix.len()..])
        } else {
            if let Some(user_prefix) = self.user_prefix.read().as_ref() {
                if message.content.starts_with(user_prefix) {
                    Some(&message.content[user_prefix.len()..])
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(content) = content {
            if let Some(command) = get_command(content) {
                error_report::catch_error(|| {
                    if let Some(ch) = message.channel() {
                        let (privilege_level, command_target, context_str) = match ch {
                            Channel::Guild(ch) => {
                                // TODO: Implement BotOwner
                                let guild = ch.read().guild().chain_err(|| "Guild not found.")?;
                                let guild = guild.read();
                                (if message.author.id == guild.owner_id {
                                    PrivilegeLevel::GuildOwner
                                } else {
                                    PrivilegeLevel::NormalUser
                                }, CommandTarget::ServerMessage,
                                format!("{} (guild #{})", guild.name, guild.id))
                            }
                            Channel::Group(group) =>
                                (PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage,
                                 format!("group #{}", group.read().channel_id)),
                            Channel::Private(_) =>
                                (PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage,
                                 "DM".to_owned()),
                            Channel::Category(category) =>
                                (PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage,
                                 format!("category #{}", category.read().id)),
                        };
                        info!("User {} used command in {}: {}",
                              message.author.tag(), context_str, message.content);
                        let ctx = DiscordContext {
                            ctx, message: &message, prefix,
                            content, privilege_level, command_target
                        };
                        command.run(&ctx, &core);
                    }
                    Ok(())
                }).ok();
            }
        }
    }
}

const STATUS_NOT_INIT: u8 = 0;
const STATUS_RUNNING : u8 = 1;
const STATUS_SHUTDOWN: u8 = 2;

struct DiscordBotData {
    shard_manager: Mutex<Option<Arc<Mutex<ShardManager>>>>, status: AtomicU8,
}
struct DiscordBot {
    token: String, core: Arc<RwLock<Option<VerifierCore>>>, data: Arc<DiscordBotData>,
}
impl DiscordBot {
    fn new(token: &str, core: Arc<RwLock<Option<VerifierCore>>>) -> Result<DiscordBot> {
        let data = Arc::new(DiscordBotData {
            shard_manager: Mutex::new(None), status: AtomicU8::new(STATUS_NOT_INIT),
        });
        Ok(DiscordBot {
            core, token: token.to_string(), data,
        })
    }
    fn start(&self) -> Result<()> {
        ensure!(self.data.status.compare_and_swap(STATUS_NOT_INIT, STATUS_RUNNING,
                                                  Ordering::Relaxed) == STATUS_NOT_INIT,
                "Discord component started twice!");
        let core = self.core.clone();
        let data = self.data.clone();
        let mut client = Client::new(&self.token, Handler {
            core: core.clone(), user_prefix: RwLock::new(None)
        })?;
        thread::Builder::new().name("discord thread".to_string()).spawn(move || {
            let core = core.read().as_ref().unwrap().clone();
            error_report::catch_error(|| {
                *data.shard_manager.lock() = Some(client.shard_manager.clone());
                match client.start_autosharded() {
                    Ok(_) | Err(SerenityError::Client(ClientError::Shutdown)) => { }
                    Err(err) => bail!(err),
                }
                Ok(())
            }).ok();
            info!("Discord connection terminated.");
            data.status.compare_and_swap(STATUS_RUNNING, STATUS_SHUTDOWN, Ordering::Relaxed);
        })?;
        Ok(())
    }
    fn shutdown(&self) -> Result<()> {
        match self.data.status.compare_and_swap(STATUS_RUNNING, STATUS_SHUTDOWN,
                                                Ordering::Relaxed) {
            STATUS_NOT_INIT => bail!("Not yet connected to Discord!"),
            STATUS_RUNNING  => {
                self.data.shard_manager.lock().as_ref().map(|x| x.lock().shutdown_all());
            },
            STATUS_SHUTDOWN => { }
            _               => unreachable!(),
        }
        Ok(())
    }
    fn is_alive(&self) -> bool {
        self.data.status.load(Ordering::Relaxed) == STATUS_RUNNING
    }
}

pub struct DiscordManager {
    core: Arc<RwLock<Option<VerifierCore>>>, bot: Option<DiscordBot>,
}
impl DiscordManager {
    pub fn new() -> DiscordManager {
        DiscordManager { core: Arc::new(RwLock::new(None)), bot: None }
    }

    pub fn set_core(&self, core: &VerifierCore) {
        *self.core.write() = Some(core.clone());
    }

    fn check_bot_dead(&mut self) {
        let is_dead = self.bot.as_ref().map_or(false, |bot| !bot.is_alive());
        if is_dead { self.bot = None }
    }
    pub fn connect(&mut self) -> Result<()> {
        self.check_bot_dead();
        if let &None = &self.bot {
            match self.core.read().as_ref().unwrap().config().get(None, ConfigKeys::DiscordToken)? {
                Some(token) => {
                    let bot = DiscordBot::new(&token, self.core.clone())?;
                    bot.start()?;
                    self.bot = Some(bot);
                }
                None => info!("No token configured for the Discord bot. Please use \
                               \"set_global token YOUR_DISCORD_TOKEN_HERE\" to configure it, then \
                               use \"connect\" to connect to Discord."),
            }
        }
        Ok(())
    }
    pub fn disconnect(&mut self) -> Result<()> {
        self.check_bot_dead();
        if let &Some(_) = &self.bot {
            self.bot.take().unwrap().shutdown()?;
        }
        Ok(())
    }
    pub fn reconnect(&mut self) -> Result<()> {
        self.disconnect()?;
        self.connect()
    }

    pub fn shutdown(&mut self) -> Result<()> {
        *self.core.write() = None;
        self.disconnect()
    }
}