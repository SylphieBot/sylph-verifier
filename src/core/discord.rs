use commands::*;
use core::CoreRef;
use core::config::*;
use core::roles::*;
use core::tasks::*;
use core::verification_channel::*;
use errors::*;
use error_report;
use parking_lot::{Mutex, RwLock};
use serenity;
use serenity::Client;
use serenity::client::bridge::gateway::ShardManager;
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::Duration;
use util;

// TODO: Check messages in verification channels on login.

struct DiscordContext<'a> {
    ctx: Context, message: &'a Message, content: &'a str, prefix: String,
    privilege_level: PrivilegeLevel, command_target: CommandTarget, command_no: usize,
    is_verification_channel: bool, delete_in: u32, tasks: TaskManager,
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
        let verify_tail = if self.is_verification_channel {
            format!("\n*This message will be deleted automatically in {}.*",
                    util::to_english_time(self.delete_in as u64))
        } else {
            String::new()
        };
        let message = self.message.channel_id.send_message(|m|
            if message.contains('\n') {
                m.content(format_args!("<@{}>\n{}{}", self.message.author.id, message, verify_tail))
            } else {
                m.content(format_args!("<@{}> {}{}", self.message.author.id, message, verify_tail))
            }
        )?;
        if self.is_verification_channel {
            self.tasks.dispatch_delayed_task(Duration::from_secs(self.delete_in as u64), move |_| {
                message.delete()?;
                Ok(())
            })
        }
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

struct CommandMutexGuard(Arc<Mutex<HashSet<UserId>>>, UserId);
impl Drop for CommandMutexGuard {
    fn drop(&mut self) {
        self.0.lock().remove(&self.1);
    }
}

#[derive(Clone)]
struct CommandMutex(Arc<Mutex<HashSet<UserId>>>);
impl CommandMutex {
    fn lock(&self, id: UserId) -> Option<CommandMutexGuard> {
        let mut lock = self.0.lock();
        if lock.contains(&id) {
            None
        } else {
            lock.insert(id);
            Some(CommandMutexGuard(self.0.clone(), id))
        }
    }

    fn shrink_to_fit(&self) {
        self.0.lock().shrink_to_fit();
    }
}

struct DiscordBotSharedData {
    config: ConfigManager, core_ref: CoreRef, roles: RoleManager, tasks: TaskManager,
    verify_channel: VerificationChannelManager, is_in_command: CommandMutex,
}

struct Handler {
    shared: Arc<DiscordBotSharedData>, status: Arc<AtomicU8>, printed_url: AtomicBool,
}
impl Handler {
    fn context_str(channel: &Channel) -> Cow<str> {
        match *channel {
            Channel::Guild(ref channel) => {
                let channel = channel.read();
                if let Some(guild) = channel.guild() {
                    let guild = guild.read();
                    format!("{} (guild #{})", guild.name, guild.id).into()
                } else {
                    format!("channel #{} in unknown guild", channel.id).into()
                }
            }
            Channel::Group(ref group) => format!("group #{}", group.read().channel_id).into(),
            Channel::Private(_) => "DM".into(),
            Channel::Category(ref category) => format!("category #{}", category.read().id).into(),
        }
    }
    fn message_info(
        user_id: UserId, channel: &Channel, bot_owner_id: Option<UserId>,
    ) -> Result<(PrivilegeLevel, CommandTarget)> {
        Ok(match *channel {
            Channel::Guild(ref channel) => {
                let guild = channel.read().guild().chain_err(|| "Guild not found.")?;
                let guild = guild.read();
                let privilege =
                    if Some(user_id) == bot_owner_id {
                        PrivilegeLevel::BotOwner
                    } else if user_id == guild.owner_id {
                        PrivilegeLevel::GuildOwner
                    } else {
                        PrivilegeLevel::NormalUser
                    };
                (privilege, CommandTarget::ServerMessage)
            }
            Channel::Group(_) | Channel::Private(_) | Channel::Category(_) =>
                (PrivilegeLevel::NormalUser, CommandTarget::PrivateMessage),
        })
    }

    fn start_command_thread(
        &self, ctx: Context, message: Message, channel: Channel, guild_id: Option<GuildId>,
        content: String, command: &'static Command, prefix: String,
    ) -> Result<()> {
        let command_no = util::command_id();
        let head = format!("{} in {}", message.author.tag(), Self::context_str(&channel));
        debug!("Assigning ID #{} to command from {}: {:?}", command_no, head, message.content);

        let core_ref = self.shared.core_ref.clone();
        let bot_owner_id = self.shared.config.get(None, ConfigKeys::BotOwnerId)?.map(UserId);
        let is_in_command = self.shared.is_in_command.clone();

        let (privilege_level, command_target) =
            Self::message_info(message.author.id, &channel, bot_owner_id)?;
        let (is_verification_channel, delete_in) = match guild_id {
            Some(guild_id) => (
                self.shared.verify_channel.is_verification_channel(guild_id, message.channel_id)?,
                self.shared.config.get(Some(guild_id), ConfigKeys::VerificationChannelDeleteSeconds)?,
            ),
            None => (false, 0),
        };
        let tasks = self.shared.tasks.clone();

        thread::Builder::new().name(format!("command #{}", command_no)).spawn(move || {
            error_report::catch_error(move || {
                info!("{}: {}", head, message.content);
                let ctx = DiscordContext {
                    ctx, message: &message, prefix, content: &content,
                    privilege_level, command_target, command_no,
                    is_verification_channel, delete_in, tasks,
                };
                if let Some(_lock) = is_in_command.lock(message.author.id) {
                    core_ref.run_command(command, &ctx);
                } else {
                    ctx.respond(&format!(
                        "<@{}> You are already running a command. Please wait for it \
                         to finish running, then try again.", message.author.id,
                    ))?;
                };
                debug!("Command #{} completed.", command_no);
                Ok(())
            }).ok();
        })?;
        Ok(())
    }

    fn on_guild_remove(&self, guild_id: GuildId) {
        self.shared.roles.on_guild_remove(guild_id);
        self.shared.config.on_guild_remove(guild_id);
        self.shared.verify_channel.on_guild_remove(guild_id);
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
        if !self.printed_url.compare_and_swap(false, true, Ordering::Relaxed) {
            let permissions = Permissions::MANAGE_ROLES | Permissions::MANAGE_NICKNAMES |
                              Permissions::READ_MESSAGES | Permissions::SEND_MESSAGES |
                              Permissions::MANAGE_MESSAGES | Permissions::READ_MESSAGE_HISTORY;
            info!("Add bot link: \
                   https://discordapp.com/oauth2/authorize?client_id={}&permissions={}&scope=bot",
                  ready.user.id, permissions.bits());
        }

        error_report::catch_error(||
            self.shared.verify_channel.check_verification_channels_ready(&ready)
        ).ok();
    }

    fn message(&self, ctx: Context, message: Message) {
        let channel = match message.channel() {
            Some(channel) => channel,
            None => return,
        };
        let guild_id = match channel {
            Channel::Guild(ref channel) => Some(channel.read().guild_id),
            _ => None,
        };
        let user_id = serenity::CACHE.read().user.id;

        if let Some(guild_id) = guild_id {
            error_report::catch_error(|| {
                self.shared.roles.check_roles_update_msg(guild_id, message.author.id)?;
                if message.author.id != user_id {
                    self.shared.verify_channel.check_verification_channel_msg(guild_id, &message)?;
                }
                Ok(())
            }).ok();
        }

        // Process commands.
        let prefix = match error_report::catch_error(||
            self.shared.config.get(None, ConfigKeys::CommandPrefix)
        ) {
            Ok(prefix) => prefix,
            Err(_) => return,
        };

        let content = if message.content.starts_with(&prefix) {
            Some(message.content[prefix.len()..].to_owned())
        } else {
            let user_prefix = format!("<@{}> ", user_id);
            if message.content.starts_with(&user_prefix) {
                Some(message.content[user_prefix.len()..].to_owned())
            } else {
                None
            }
        };

        if let Some(content) = content {
            if let Some(command) = get_command(&content) {
                error_report::catch_error(||
                    self.start_command_thread(ctx, message, channel, guild_id,
                                              content, command, prefix)
                ).ok();
            }
        }
    }

    fn guild_member_addition(&self, _: Context, guild_id: GuildId, member: Member) {
        error_report::catch_error(||
            self.shared.roles.check_roles_update_join(guild_id, member)
        ).ok();
    }

    fn guild_delete(&self, _: Context, guild: PartialGuild, _: Option<Arc<RwLock<Guild>>>) {
        self.on_guild_remove(guild.id);
    }
    fn guild_unavailable(&self, _: Context, guild_id: GuildId) {
        self.on_guild_remove(guild_id);
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
            shared: self.shared.clone(), status: self.status.clone(),
            printed_url: AtomicBool::new(false),
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
        verify_channel: VerificationChannelManager,
    ) -> DiscordManager {
        DiscordManager {
            bot: Mutex::new(BotStatus::NotConnected), shutdown: AtomicBool::new(false),
            shared: Arc::new(DiscordBotSharedData {
                config, core_ref, roles, tasks, verify_channel,
                is_in_command: CommandMutex(Arc::new(Mutex::new(HashSet::new()))),
            }),
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

    pub fn on_cleanup_tick(&self) {
        self.shared.is_in_command.shrink_to_fit()
    }
}