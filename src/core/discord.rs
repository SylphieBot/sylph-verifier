use commands::*;
use core::CoreRef;
use core::config::*;
use core::roles::*;
use core::tasks::*;
use errors::*;
use error_report;
use parking_lot::{Mutex, RwLock};
use serenity::Client;
use serenity::client::bridge::gateway::ShardManager;
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::borrow::Cow;
use std::cmp::max;
use std::mem::drop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use util;
use util::ConcurrentCache;

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
const STATUS_STARTING: u8 = 1;
const STATUS_RUNNING : u8 = 2;
const STATUS_SHUTDOWN: u8 = 3;
const STATUS_DROPPED : u8 = 4;

struct DiscordBotSharedData {
    config: ConfigManager, core_ref: CoreRef, roles: RoleManager, tasks: TaskManager,
}

struct Handler {
    user_prefix: RwLock<Option<String>>, is_in_command: ConcurrentCache<UserId, Mutex<()>>,
    shared: Arc<DiscordBotSharedData>, status: Arc<AtomicU8>,
}
impl Handler {
    fn context_str(message: &Message) -> Cow<str> {
        if let Some(channel) = message.channel() {
            match channel {
                Channel::Guild(channel) => {
                    let channel = channel.read();
                    if let Some(guild) = channel.guild() {
                        let guild = guild.read();
                        format!("{} (guild #{})", guild.name, guild.id).into()
                    } else {
                        format!("channel #{} in unknown guild", channel.id).into()
                    }
                }
                Channel::Group(group) =>
                    format!("group #{}", group.read().channel_id).into(),
                Channel::Private(_) =>
                    "DM".into(),
                Channel::Category(category) =>
                    format!("category #{}", category.read().id).into(),
            }
        } else {
            "unknown location".into()
        }
    }
    fn message_info(
        message: &Message, channel: Channel, bot_owner_id: Option<UserId>,
    ) -> Result<(PrivilegeLevel, CommandTarget)> {
        Ok(match channel {
            Channel::Guild(channel) => {
                let guild = channel.read().guild().chain_err(|| "Guild not found.")?;
                let guild = guild.read();
                let privilege =
                    if Some(message.author.id) == bot_owner_id {
                        PrivilegeLevel::BotOwner
                    } else if message.author.id == guild.owner_id {
                        PrivilegeLevel::GuildOwner
                    } else {
                        PrivilegeLevel::NormalUser
                    };
                (privilege, CommandTarget::ServerMessage)
            }
            Channel::Group(_) | Channel::Private(_) =>
                (PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage),
            Channel::Category(_) => bail!("Received message in category channel."),
        })
    }

    fn start_command_thread(
        &self, ctx: Context, message: Message,
        content: String, command: &'static Command, prefix: String,
    ) -> Result<()> {
        let command_no = util::command_id();
        let head = format!("{} in {}", message.author.tag(), Self::context_str(&message));
        debug!("Assigning ID #{} to command from {}: {:?}", command_no, head, message.content);

        let core_ref = self.shared.core_ref.clone();
        let bot_owner_id = self.shared.config.get(None, ConfigKeys::BotOwnerId)?.map(UserId);
        let mutex = self.is_in_command.read(&message.author.id, || Ok(Mutex::new(())))?;
        thread::Builder::new().name(format!("command #{}", command_no)).spawn(move || {
            error_report::catch_error(move || {
                if let Some(channel) = message.channel() {
                    let (privilege_level, command_target) =
                        Self::message_info(&message, channel, bot_owner_id)?;
                    info!("{}: {}", head, message.content);
                    let ctx = DiscordContext {
                        ctx, message: &message, prefix, content: &content,
                        privilege_level, command_target, command_no,
                    };
                    if let Some(lock) = mutex.try_lock() {
                        core_ref.run_command(command, &ctx);
                        drop(lock);
                    } else {
                        ctx.respond(&format!(
                            "<@{}> You are already running a command. Please wait for it \
                             to finish running, then try again.", message.author.id,
                        ))?;
                    };
                    debug!("Command #{} completed.", command_no);
                }
                Ok(())
            }).ok();
        })?;
        Ok(())
    }
}
impl Drop for Handler {
    fn drop(&mut self) {
        info!("Discord event handler shut down.");
        self.status.compare_and_swap(STATUS_SHUTDOWN, STATUS_DROPPED, Ordering::Relaxed);
    }
}
impl EventHandler for Handler {
    fn ready(&self, _: Context, ready: Ready) {
        *self.user_prefix.write() = Some(format!("<@{}> ", ready.user.id))
    }

    fn message(&self, ctx: Context, message: Message) {
        // Check for roles update
        error_report::catch_error(|| {
            let guild_id = if let Some(channel) = message.channel() {
                match channel {
                    Channel::Guild(channel) => {
                        let guild = channel.read().guild().chain_err(|| "Guild not found.")?;
                        let guild = guild.read();
                        guild.id
                    }
                    _ => return Ok(()),
                }
            } else {
                return Ok(())
            };

            let set_roles_on_join =
                self.shared.config.get(None, ConfigKeys::AllowEnableAutoUpdate)? &&
                self.shared.config.get(Some(guild_id), ConfigKeys::EnableAutoUpdate)?;
            if set_roles_on_join {
                let auto_update_cooldown = max(
                    self.shared.config.get(None, ConfigKeys::MinimumAutoUpdateCooldownSeconds)?,
                    self.shared.config.get(Some(guild_id), ConfigKeys::AutoUpdateCooldownSeconds)?
                );
                let shared = self.shared.clone();
                let user_id = message.author.id;
                self.shared.tasks.dispatch_task(move |_| {
                    shared.roles.update_user_with_cooldown(
                        guild_id, user_id, auto_update_cooldown, false
                    ).cmd_ok()?;
                    Ok(())
                })
            }
            Ok(())
        }).ok();

        // Process commands.
        let prefix = error_report::catch_error(||
            self.shared.config.get(None, ConfigKeys::CommandPrefix)
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
                error_report::catch_error(||
                    self.start_command_thread(ctx, message, content, command, prefix)
                ).ok();
            }
        }
    }

    fn guild_member_addition(&self, _: Context, guild_id: GuildId, member: Member) {
        error_report::catch_error(|| {
            let set_roles_on_join =
                self.shared.config.get(None, ConfigKeys::AllowSetRolesOnJoin)? &&
                self.shared.config.get(Some(guild_id), ConfigKeys::SetRolesOnJoin)?;
            if set_roles_on_join {
                let shared = self.shared.clone();
                let user_id = member.user.read().id;
                self.shared.tasks.dispatch_task(move |_| {
                    shared.roles.update_user_with_cooldown(guild_id, user_id, 0, false).cmd_ok()?;
                    Ok(())
                })
            }
            Ok(())
        }).ok();
    }
}

struct DiscordBot {
    token: String, status: Arc<AtomicU8>, shared: Arc<DiscordBotSharedData>,
    shard_manager: Mutex<Option<Arc<Mutex<ShardManager>>>>,
}
impl DiscordBot {
    fn new(token: &str, shared: Arc<DiscordBotSharedData>) -> Result<DiscordBot> {
        Ok(DiscordBot {
            token: token.to_string(), status: Arc::new(AtomicU8::new(STATUS_NOT_INIT)),
            shard_manager: Mutex::new(None), shared,
        })
    }
    fn start(&self) -> Result<()> {
        ensure!(self.status.compare_and_swap(STATUS_NOT_INIT, STATUS_STARTING,
                                             Ordering::Relaxed) == STATUS_NOT_INIT,
                "Discord component already started!");
        let mut client = Client::new(&self.token, Handler {
            user_prefix: RwLock::new(None), is_in_command: ConcurrentCache::new(),
            shared: self.shared.clone(), status: self.status.clone(),
        })?;
        *self.shard_manager.lock() = Some(client.shard_manager.clone());
        ensure!(self.status.compare_and_swap(STATUS_STARTING, STATUS_RUNNING,
                                             Ordering::Relaxed) == STATUS_STARTING,
                "Internal error: DiscordBot not in STATUS_STARTING!");
        thread::Builder::new().name("discord thread".to_string()).spawn(move || {
            error_report::catch_error(|| {
                match client.start_autosharded() {
                    Ok(_) | Err(SerenityError::Client(ClientError::Shutdown)) => Ok(()),
                    Err(err) => bail!(err),
                }
            }).ok();
        })?;
        Ok(())
    }
    fn shutdown(&self) -> Result<()> {
        match self.status.compare_and_swap(STATUS_RUNNING, STATUS_SHUTDOWN,
                                           Ordering::Relaxed) {
            STATUS_NOT_INIT => bail!("Bot not yet started!"),
            STATUS_STARTING => bail!("Bot not yet fully started!"),
            STATUS_RUNNING  => {
                self.shard_manager.lock().as_ref().unwrap().lock().shutdown_all();
                while self.status.load(Ordering::Relaxed) != STATUS_DROPPED {
                    thread::yield_now()
                }
            },
            STATUS_SHUTDOWN => { }
            _               => unreachable!(),
        }
        Ok(())
    }
    fn is_alive(&self) -> bool {
        self.status.load(Ordering::Relaxed) == STATUS_RUNNING
    }
}

enum BotStatus {
    NotConnected, Connected(DiscordBot),
}
impl BotStatus {
    fn check_bot_dead(&mut self) {
        if let BotStatus::Connected(ref bot) = *self {
            if !bot.is_alive() {
                *self = BotStatus::NotConnected
            }
        }
    }
    fn connect_internal(&mut self, shared: &Arc<DiscordBotSharedData>) -> Result<()> {
        self.check_bot_dead();
        if let BotStatus::NotConnected = *self {
            match shared.config.get(None, ConfigKeys::DiscordToken)? {
                Some(token) => {
                    let bot = DiscordBot::new(&token, shared.clone())?;
                    bot.start()?;
                    *self = BotStatus::Connected(bot);
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
        if let BotStatus::Connected(ref bot) = *self {
            bot.shutdown()?;
            *self = BotStatus::NotConnected
        }
        Ok(())
    }
}

pub struct DiscordManager {
    bot: Mutex<BotStatus>, shutdown: AtomicBool, shared: Arc<DiscordBotSharedData>,
}
impl DiscordManager {
    pub(in ::core) fn new(
        config: ConfigManager, core_ref: CoreRef, roles: RoleManager, tasks: TaskManager,
    ) -> DiscordManager {
        DiscordManager {
            bot: Mutex::new(BotStatus::NotConnected), shutdown: AtomicBool::new(false),
            shared: Arc::new(DiscordBotSharedData { config, core_ref, roles, tasks }),
        }
    }

    pub fn connect(&self) -> Result<()> {
        if !self.shutdown.load(Ordering::Relaxed) {
            let mut bot = self.bot.lock();
            bot.connect_internal(&self.shared)?;
        }
        Ok(())
    }
    pub fn disconnect(&self) -> Result<()> {
        if !self.shutdown.load(Ordering::Relaxed) {
            let mut bot = self.bot.lock();
            bot.disconnect_internal()?;
        }
        Ok(())
    }
    pub fn reconnect(&self) -> Result<()> {
        if !self.shutdown.load(Ordering::Relaxed) {
            let mut bot = self.bot.lock();
            bot.disconnect_internal()?;
            bot.connect_internal(&self.shared)?;
        }
        Ok(())
    }
    pub(in ::core) fn shutdown(&self) -> Result<()> {
        if !self.shutdown.compare_and_swap(false, true, Ordering::Relaxed) {
            let mut bot = self.bot.lock();
            bot.disconnect_internal()?;
        }
        Ok(())
    }
}