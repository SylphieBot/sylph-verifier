use commands::*;
use core::CommandSender;
use core::config::*;
use errors::*;
use error_report;
use parking_lot::{Mutex, RwLock};
use serenity::Client;
use serenity::client::bridge::gateway::ShardManager;
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::mem::drop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use util;

// TODO: Implement SetOnJoin and UpdateOnMessage

struct DiscordContext<'a> {
    ctx: Context, message: &'a Message, content: &'a str, prefix: String,
    privilege_level: PrivilegeLevel, command_target: CommandTarget, command_no: usize,
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
    fn respond(&self, message: &str) -> Result<()> {
        for line in message.split('\n') {
            debug!(target: "$raw", "[Command #{}] {}", self.command_no, line);
        }
        self.message.channel_id.send_message(|m|
            if message.contains('\n') {
                m.content(format_args!("<@{}>\n{}", self.message.author.id, message))
            } else {
                m.content(format_args!("<@{}> {}", self.message.author.id, message))
            }
        )?;
        Ok(())
    }
    fn discord_context(&self) -> Option<(&Context, &Message)> {
        Some((&self.ctx, self.message))
    }
}

const STATUS_NOT_INIT: u8 = 0;
const STATUS_RUNNING : u8 = 1;
const STATUS_SHUTDOWN: u8 = 2;
const STATUS_DROPPED : u8 = 3;

struct DiscordBotData {
    shard_manager: Mutex<Option<Arc<Mutex<ShardManager>>>>, status: AtomicU8,
}

struct Handler {
    config: ConfigManager, cmd_sender: CommandSender, user_prefix: RwLock<Option<String>>,
    data: Arc<DiscordBotData>,
}
impl Handler {
    fn message_info(
        message: &Message, channel: Channel
    ) -> Result<(PrivilegeLevel, CommandTarget, String)> {
        Ok(match channel {
            Channel::Guild(channel) => {
                // TODO: Implement BotOwner
                let guild = channel.read().guild().chain_err(|| "Guild not found.")?;
                let guild = guild.read();
                let privilege = if message.author.id == guild.owner_id {
                    PrivilegeLevel::GuildOwner
                } else {
                    PrivilegeLevel::NormalUser
                };
                (
                    privilege, CommandTarget::ServerMessage,
                    format!("{} (guild #{})", guild.name, guild.id)
                )
            }
            Channel::Group(group) => (
                PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage,
                format!("group #{}", group.read().channel_id)
            ),
            Channel::Private(_) => (
                PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage,
                "DM".to_owned()
            ),
            Channel::Category(category) => (
                PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage,
                format!("category #{}", category.read().id)
            ),
        })
    }
}
impl Drop for Handler {
    fn drop(&mut self) {
        info!("Discord event handler shut down.");
        self.data.status.compare_and_swap(STATUS_SHUTDOWN, STATUS_DROPPED, Ordering::Relaxed);
    }
}
impl EventHandler for Handler {
    fn ready(&self, _: Context, ready: Ready) {
        *self.user_prefix.write() = Some(format!("<@{}> ", ready.user.id))
    }

    fn message(&self, ctx: Context, message: Message) {
        let prefix = error_report::catch_error(||
            self.config.get(None, ConfigKeys::CommandPrefix)
        );
        let prefix = match prefix {
            Ok(prefix) => prefix,
            Err(_) => return,
        };

        let content = if message.content.starts_with(&prefix) {
            Some(message.content[prefix.len()..].to_owned())
        } else if let Some(user_prefix) = self.user_prefix.read().as_ref() {
            if message.content.starts_with(user_prefix) {
                Some(message.content[user_prefix.len()..].to_owned())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(content) = content {
            if let Some(command) = get_command(&content) {
                let command_no = util::command_id();
                let cmd_sender = self.cmd_sender.clone();
                error_report::catch_error(move || {
                    thread::Builder::new().name(format!("command #{}", command_no)).spawn(move || {
                        error_report::catch_error(move || {
                            if let Some(channel) = message.channel() {
                                let (privilege_level, command_target, context_str) =
                                    Self::message_info(&message, channel)?;
                                let head = format!("{} in {}", message.author.tag(), context_str);
                                info!("{}: {}", head, message.content);
                                debug!("Assigning ID #{} to commad from {}: \"{}\"",
                                        command_no, head, message.content);
                                let ctx = DiscordContext {
                                    ctx, message: &message, prefix, content: &content,
                                    privilege_level, command_target, command_no,
                                };
                                cmd_sender.run_command(command, &ctx)
                            }
                            Ok(())
                        }).ok();
                    })?;
                    Ok(())
                }).ok();
            }
        }
    }
}

struct DiscordBot {
    token: String, config: ConfigManager, cmd_sender: CommandSender, data: Arc<DiscordBotData>,
}
impl DiscordBot {
    fn new(token: &str, config: ConfigManager, cmd_sender: CommandSender) -> Result<DiscordBot> {
        let data = Arc::new(DiscordBotData {
            shard_manager: Mutex::new(None), status: AtomicU8::new(STATUS_NOT_INIT),
        });
        Ok(DiscordBot {
            config, cmd_sender, token: token.to_string(), data,
        })
    }
    fn start(&self) -> Result<()> {
        ensure!(self.data.status.compare_and_swap(STATUS_NOT_INIT, STATUS_RUNNING,
                                                  Ordering::Relaxed) == STATUS_NOT_INIT,
                "Discord component already started!");
        let data = self.data.clone();
        let mut client = Client::new(&self.token, Handler {
            config: self.config.clone(), cmd_sender: self.cmd_sender.clone(),
            user_prefix: RwLock::new(None), data: data.clone(),
        })?;
        thread::Builder::new().name("discord thread".to_string()).spawn(move || {
            error_report::catch_error(|| {
                *data.shard_manager.lock() = Some(client.shard_manager.clone());
                drop(data);
                match client.start_autosharded() {
                    Ok(_) | Err(SerenityError::Client(ClientError::Shutdown)) => Ok(()),
                    Err(err) => bail!(err),
                }
            }).ok();
        })?;
        Ok(())
    }
    fn shutdown(&self) -> Result<()> {
        match self.data.status.compare_and_swap(STATUS_RUNNING, STATUS_SHUTDOWN,
                                                Ordering::Relaxed) {
            STATUS_NOT_INIT => bail!("Not yet connected to Discord!"),
            STATUS_RUNNING  => {
                self.data.shard_manager.lock().as_ref().map(|x| x.lock().shutdown_all());
                while self.data.status.load(Ordering::Relaxed) != STATUS_DROPPED {
                    thread::yield_now()
                }
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
    config: ConfigManager, cmd_sender: CommandSender, bot: Option<DiscordBot>,
    shutdown: AtomicBool,
}
impl DiscordManager {
    pub(in ::core) fn new(config: ConfigManager, cmd_sender: CommandSender) -> DiscordManager {
        DiscordManager { config, cmd_sender, bot: None, shutdown: AtomicBool::new(false) }
    }

    fn check_bot_dead(&mut self) {
        let is_dead = self.bot.as_ref().map_or(false, |bot| !bot.is_alive());
        if is_dead { self.bot = None }
    }

    fn connect_internal(&mut self) -> Result<()> {
        self.check_bot_dead();
        if self.bot.is_none() {
            match self.config.get(None, ConfigKeys::DiscordToken)? {
                Some(token) => {
                    let bot = DiscordBot::new(&token,
                                              self.config.clone(), self.cmd_sender.clone())?;
                    bot.start()?;
                    self.bot = Some(bot);
                }
                None => info!("No token configured for the Discord bot. Please use \
                               \"set_global discord_token YOUR_DISCORD_TOKEN_HERE\" to \
                               configure it."),
            }
        }
        Ok(())
    }
    fn disconnect_internal(&mut self) -> Result<()> {
        self.check_bot_dead();
        if self.bot.is_some() {
            self.bot.take().unwrap().shutdown()?;
        }
        Ok(())
    }

    pub fn connect(&mut self) -> Result<()> {
        if !self.shutdown.load(Ordering::Relaxed) {
            self.connect_internal()?;
        }
        Ok(())
    }
    pub fn disconnect(&mut self) -> Result<()> {
        if !self.shutdown.load(Ordering::Relaxed) {
            self.disconnect_internal()?;
        }
        Ok(())
    }
    pub fn reconnect(&mut self) -> Result<()> {
        if !self.shutdown.load(Ordering::Relaxed) {
            self.disconnect_internal()?;
            self.connect_internal()?;
        }
        Ok(())
    }
    pub fn shutdown(&mut self) -> Result<()> {
        if !self.shutdown.compare_and_swap(false, true, Ordering::Relaxed) {
            self.disconnect_internal()?;
        }
        Ok(())
    }
}