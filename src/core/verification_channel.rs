use database::*;
use core::config::*;
use errors::*;
use serenity::model::prelude::*;
use std::sync::Arc;
use std::time::{SystemTime, Duration};
use util::ConcurrentCache;

// TODO: Handle the verification channel being deleted.

struct VerificationChannelManagerData {
    config: ConfigManager, database: Database,
    channel_cache: ConcurrentCache<GuildId, Option<(ChannelId, MessageId)>>,
}

#[derive(Clone)]
pub struct VerificationChannelManager(Arc<VerificationChannelManagerData>);
impl VerificationChannelManager {
    pub fn new(config: ConfigManager, database: Database) -> VerificationChannelManager {
        let db_ref_update = database.clone();
        VerificationChannelManager(Arc::new(VerificationChannelManagerData {
            config, database,
            channel_cache: ConcurrentCache::new(move |&guild_id| {
                Self::get_verification_channel(&db_ref_update, guild_id)
            }),
        }))
    }

    fn get_verification_channel(
        database: &Database, guild_id: GuildId
    ) -> Result<Option<(ChannelId, MessageId)>> {
        database.connect()?.query(
            "SELECT discord_channel_id, header_message_id FROM verification_channel_info \
             WHERE discord_guild_id = ?1", guild_id
        ).get_opt::<(ChannelId, MessageId)>()
    }
    pub fn is_verification_channel(
        &self, guild_id: GuildId, channel_id: ChannelId
    ) -> Result<bool> {
        Ok(self.0.channel_cache.read(&guild_id)?.map(|x| x.0) == Some(channel_id))
    }
    fn set_verification_channel(
        &self, guild_id: GuildId, channel_id: ChannelId, message_id: MessageId,
    ) -> Result<()> {
        self.0.database.connect()?.execute(
            "REPLACE INTO verification_channel_info (\
                 discord_guild_id, discord_channel_id, header_message_id\
             ) VALUES (?1, ?2, ?3)", (guild_id, channel_id, message_id),
        )?;
        *self.0.channel_cache.write(&guild_id)? = Some((channel_id, message_id));
        Ok(())
    }

    pub fn check_verification_channel_msg(
        &self, guild_id: GuildId, message: &Message
    ) -> Result<()> {
        if self.is_verification_channel(guild_id, message.channel_id)? {
            message.delete().map_err(Error::from).discord_to_cmd().cmd_ok()?;
        }
        Ok(())
    }

    fn remove_messages(&self, channel_id: ChannelId, ignore: Option<MessageId>) -> Result<()> {
        let mut tries = 0;
        loop {
            let messages = channel_id.messages(|x| x.limit(100))?;
            let messages: Vec<MessageId> =
                messages.iter().filter(|x| Some(x.id) != ignore).map(|x| x.id).collect();
            if messages.len() == 0 {
                break
            }
            channel_id.delete_messages(messages)?;
            tries += 1;
            cmd_ensure!(tries < 5, "Could not delete all messages in 5 tries.");
        }
        Ok(())
    }
    fn delete_old_messages(&self, guild_id: GuildId) -> Result<()> {
        if let Some((channel_id, message_id)) = *self.0.channel_cache.read(&guild_id)? {
            trace!("Removing missed messages in verification channel for {}.", guild_id);
            self.remove_messages(channel_id, Some(message_id))?;
        }
        Ok(())
    }

    pub fn check_guild_create(&self, guild_id: GuildId) -> Result<()> {
        self.delete_old_messages(guild_id).discord_to_cmd().cmd_ok()
    }
    pub fn check_verification_channels_ready(&self, ready: &Ready, ) -> Result<()> {
        for guild in &ready.guilds {
            match *guild {
                GuildStatus::OnlineGuild(ref guild) =>
                    self.delete_old_messages(guild.id).discord_to_cmd().cmd_ok()?,
                GuildStatus::OnlinePartialGuild(ref guild) =>
                    self.delete_old_messages(guild.id).discord_to_cmd().cmd_ok()?,
                _ => { }
            }
        }
        Ok(())
    }

    pub fn verify_instructions(&self) -> Result<String> {
        if let Some(place_id) = self.0.config.get(None, ConfigKeys::PlaceID)? {
            Ok(format!("To verify your Roblox account with your Discord account, please \
                        follow these instructions:\n\
                        • Visit <https://roblox.com/--place?id={}> as the account you want to \
                          verify as.\n\
                        • Type the command it shows in the oval box into this channel.",
                       place_id))
        } else {
            error!("No place ID set! Please upload the place file to Roblox, and use \
                    \"set_global place_id [your place id]\".");
            cmd_error!("No place ID set. Please ask the bot owner to fix this.")
        }
    }
    fn intro_message(&self, guild_id: GuildId) -> Result<String> {
        let verify_intro =
            self.0.config.get(Some(guild_id), ConfigKeys::VerificationChannelIntro)?;
        let intro_space = match verify_intro {
            Some(ref x) => if x.contains('\n') { "\n\n" } else { " " },
            None => "",
        };

        let verify_footer =
            self.0.config.get(Some(guild_id), ConfigKeys::VerificationChannelFooter)?;
        let footer_space = match verify_footer {
            Some(_) => "\n\n",
            None => "",
        };

        Ok(format!("{}{}{}{}{}",
                   verify_intro.unwrap_or_else(|| String::new()), intro_space,
                   self.verify_instructions()?,
                   footer_space, verify_footer.unwrap_or_else(|| String::new())))
    }
    pub fn setup_check(&self, _: GuildId, channel_id: ChannelId) -> Result<()> {
        let messages = channel_id.messages(|x| x.limit(51))?;
        cmd_ensure!(messages.len() <= 50,
                    "Cannot create verification channel in channels containing \
                     more than 50 messages.");

        let cutoff = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 7);
        for message in messages {
            cmd_ensure!(cutoff < message.timestamp.into(),
                        "Cannot create verification channel in channels containing \
                         messages older than 1 week. Please manually delete any such \
                         messages.");
        }
        Ok(())
    }
    pub fn setup(&self, guild_id: GuildId, channel_id: ChannelId) -> Result<()> {
        self.setup_check(guild_id, channel_id)?;
        self.remove_messages(channel_id, None)?;

        let message_text = self.intro_message(guild_id)?;
        let message = channel_id.send_message(|x| x.content(message_text))?;
        self.set_verification_channel(guild_id, channel_id, message.id)?;

        Ok(())
    }
    fn update_message_guild(&self, guild_id: GuildId) -> Result<()> {
        if let Some((channel_id, message_id)) = *self.0.channel_cache.read(&guild_id)? {
            let message_text = self.intro_message(guild_id)?;
            channel_id.edit_message(message_id, |x| x.content(message_text))?;
        }
        Ok(())
    }
    fn update_all_messages(&self) -> Result<()> {
        let guilds = self.0.database.connect()?.query(
            "SELECT discord_guild_id FROM verification_channel_info", (),
        ).get_all::<GuildId>()?;
        for guild_id in guilds {
            self.update_message_guild(guild_id).discord_to_cmd().cmd_ok()?;
        }
        Ok(())
    }
    pub fn update(&self, guild_id: Option<GuildId>) -> Result<()> {
        if let Some(guild_id) = guild_id {
            self.update_message_guild(guild_id).discord_to_cmd().cmd_ok()
        } else {
            self.update_all_messages()
        }
    }
    pub fn remove(&self, guild_id: GuildId) -> Result<()> {
        self.0.database.connect()?.execute(
            "DELETE FROM verification_channel_info WHERE discord_guild_id = ?1", guild_id,
        )?;
        *self.0.channel_cache.write(&guild_id)? = None;
        Ok(())
    }

    pub fn on_cleanup_tick(&self) {
        self.0.channel_cache.shrink_to_fit();
    }
    pub fn on_guild_remove(&self, guild: GuildId) {
        self.0.channel_cache.remove(&guild);
    }
}