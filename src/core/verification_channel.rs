use database::*;
use core::config::*;
use errors::*;
use serenity::model::prelude::*;
use std::sync::Arc;
use std::time::{SystemTime, Duration};
use util::ConcurrentCache;

struct VerificationChannelManagerData {
    config: ConfigManager, database: Database,
    verification_channel_cache: ConcurrentCache<GuildId, Option<(ChannelId, MessageId)>>,
}

#[derive(Clone)]
pub struct VerificationChannelManager(Arc<VerificationChannelManagerData>);
impl VerificationChannelManager {
    pub fn new(config: ConfigManager, database: Database) -> VerificationChannelManager {
        let db_ref_update = database.clone();
        VerificationChannelManager(Arc::new(VerificationChannelManagerData {
            config, database,
            verification_channel_cache: ConcurrentCache::new(move |&guild_id| {
                Self::get_verification_channel(&db_ref_update, guild_id)
            }),
        }))
    }

    fn get_verification_channel(
        database: &Database, guild_id: GuildId
    ) -> Result<Option<(ChannelId, MessageId)>> {
        database.connect()?.query_cached(
            "SELECT discord_channel_id, header_message_id FROM verification_channel_info \
             WHERE discord_guild_id = ?1", guild_id
        ).get_opt::<(ChannelId, MessageId)>()
    }
    pub fn is_verification_channel(
        &self, guild_id: GuildId, channel_id: ChannelId
    ) -> Result<bool> {
        Ok(self.0.verification_channel_cache.read(&guild_id)?.map(|x| x.0) == Some(channel_id))
    }
    fn set_verification_channel(
        &self, guild_id: GuildId, channel_id: ChannelId, message_id: MessageId,
    ) -> Result<()> {
        self.0.database.connect()?.execute(
            "REPLACE INTO verification_channel_info (\
                 discord_guild_id, discord_channel_id, header_message_id\
             ) VALUES (?1, ?2, ?3)", (guild_id, channel_id, message_id),
        )?;
        *self.0.verification_channel_cache.write(&guild_id)? = Some((channel_id, message_id));
        Ok(())
    }

    pub fn check_verification_channel_msg(
        &self, guild_id: GuildId, message: &Message
    ) -> Result<()> {
        if self.is_verification_channel(guild_id, message.channel_id)? {
            message.delete()?;
        }
        Ok(())
    }

    fn intro_message(&self, guild_id: GuildId) -> Result<String> {
        let verify_intro = self.0.config.get(Some(guild_id), ConfigKeys::VerificationChannelIntro)?;
        let space = match verify_intro {
            Some(ref x) => if x.contains('\n') { "\n\n" } else { " " },
            None => "",
        };
        if let Some(place_id) = self.0.config.get(Some(guild_id), ConfigKeys::PlaceID)? {
            Ok(format!("{}{}To verify your Roblox account with your Discord account, please \
                        follow the following instructions:\n\
                        • Visit the verification place at <https://roblox.com/--place?id={}> as \
                          the account you want to verify as.\n\
                        • Enter the command displayed in the oval box there into this channel.\n\
                        • Your roles will be set according to your Roblox account. If they are \
                          not, please contact the server operators.",
                       verify_intro.unwrap_or_else(|| String::new()), space,
                       place_id))
        } else {
            error!("No place ID set! Please upload the place file to Roblox, and use \
                    \"set_global place_id [your place id]\".");
            cmd_error!("No place ID set. Please ask the bot owner to fix this.")
        }
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

        let mut tries = 0;
        loop {
            let messages = channel_id.messages(|x| x.limit(100))?;
            if messages.len() == 0 {
                break
            }
            channel_id.delete_messages(messages.iter().map(|x| x.id))?;
            tries += 1;
            cmd_ensure!(tries < 5, "Could not delete all messages in 5 tries.");
        }

        let message_text = self.intro_message(guild_id)?;
        let message = channel_id.send_message(|x| x.content(message_text))?;
        self.set_verification_channel(guild_id, channel_id, message.id)?;

        Ok(())
    }

    pub fn on_cleanup_tick(&self) {
        self.0.verification_channel_cache.shrink_to_fit();
    }
    pub fn on_guild_remove(&self, guild: GuildId) {
        self.0.verification_channel_cache.remove(&guild);
    }
}