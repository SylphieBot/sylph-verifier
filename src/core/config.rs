use core::VerifierCore;
use database::*;
use errors::*;
use parking_lot::RwLock;
use serenity::model::prelude::GuildId;
use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use util::ConcurrentCache;

pub struct ConfigKey<T: 'static> {
    enum_name: ConfigKeyName, _phantom: PhantomData<(fn(T), fn() -> T)>,
}
impl <T> Clone for ConfigKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl <T> Copy for ConfigKey<T> { }

pub enum ConfigKeys { }

fn get_db<T: FromSql>(
    conn: &DatabaseConnection, guild: Option<GuildId>, key: &str,
) -> Result<Option<T>> {
    Ok(match guild {
        Some(guild) => {
            conn.query_cached(
                "SELECT value FROM guild_config \
                WHERE discord_guild_id = ?1 AND key = ?2", (guild, key)
            ).get_opt()?
        }
        None =>
            conn.query_cached(
                "SELECT value FROM global_config WHERE key = ?1", key
            ).get_opt()?
    })
}
fn set_db<T: ToSql>(
    conn: &DatabaseConnection, guild: Option<GuildId>, key: &str, value: &T,
) -> Result<()> {
    match guild {
        Some(guild) => {
            conn.execute_cached(
                "REPLACE INTO guild_config (discord_guild_id, key, value) \
                 VALUES (?1, ?2, ?3)", (guild, key, value))?;
        }
        None => {
            conn.execute_cached(
                "REPLACE INTO global_config (key, value) VALUES (?1, ?2)", (key, value))?;
        }
    }
    Ok(())
}
fn reset_db(
    conn: &DatabaseConnection, guild: Option<GuildId>, key: &str,
) -> Result<()> {
    match guild {
        Some(guild) => {
            conn.execute_cached(
                "DELETE FROM guild_config \
                 WHERE discord_guild_id = ?1 AND key = ?2", (guild, key))?;
        }
        None => {
            conn.execute_cached(
                "DELETE FROM global_config WHERE key = ?1", key)?;
        }
    }
    Ok(())
}

#[inline(never)]
fn get_db_type_panic() -> ! {
    panic!("Incorrect types in get_db!")
}

macro_rules! config_keys {
    ($($name:ident<$tp:ty>($default:expr $(, $after_update:expr)* $(,)*);)*) => {
        #[derive(Copy, Clone)]
        enum ConfigKeyName {
            $($name,)*
        }

        #[allow(non_snake_case)]
        struct ConfigCache {
            $($name: RwLock<Option<Option<$tp>>>,)*
        }
        impl ConfigCache {
            fn new() -> ConfigCache {
                ConfigCache {
                    $($name: RwLock::new(None),)*
                }
            }

            fn init_field(
                &self, conn: &DatabaseConnection, guild: Option<GuildId>, name: ConfigKeyName,
            ) -> Result<()> {
                match name {
                    $(ConfigKeyName::$name => {
                        let is_none = { self.$name.read().is_none() };
                        if is_none {
                            let mut db_result = get_db::<$tp>(conn, guild, stringify!($name))?;
                            if guild.is_none() {
                                if let None = db_result {
                                    db_result = Some($default);
                                }
                            }
                            *self.$name.write() = Some(db_result);
                        }
                    })*
                }
                Ok(())
            }
            fn after_update(&self, core: &VerifierCore, name: ConfigKeyName) -> Result<()> {
                match name {
                    $(ConfigKeyName::$name => {
                        $({
                            let after_update: fn(&VerifierCore) -> Result<()> = $after_update;
                            after_update(core)?;
                        })*
                    })*
                }
                Ok(())
            }

            fn set<T: ToSql>(
                &self, core: &VerifierCore, conn: &DatabaseConnection, guild: Option<GuildId>,
                key: ConfigKey<T>, value: T,
            ) -> Result<()> {
                match key.enum_name {
                    $(ConfigKeyName::$name => {
                        if TypeId::of::<T>() != TypeId::of::<$tp>() {
                            get_db_type_panic()
                        } else {
                            set_db(conn, guild, stringify!($name), &value)?;

                            // This will only execute if transmute is an noop, so this is safe.
                            {
                                let mut write_ptr = self.$name.write();
                                let mut transmute: &mut Option<Option<T>> = unsafe {
                                    mem::transmute(write_ptr.deref_mut())
                                };
                                *transmute = Some(Some(value));
                            }
                        }
                    })*
                }
                self.after_update(core, key.enum_name)
            }
            fn get<T: FromSql + Clone + 'static>(
                &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>,
            ) -> Result<Option<T>> {
                self.init_field(conn, guild, key.enum_name)?;
                match key.enum_name {
                    $(ConfigKeyName::$name => {
                        if TypeId::of::<T>() != TypeId::of::<$tp>() {
                            get_db_type_panic()
                        } else {
                            // This will only execute if transmute is an noop, so this is safe.
                            let read_ptr = self.$name.read();
                            let transmute: &Option<Option<T>> = unsafe {
                                mem::transmute(read_ptr.deref())
                            };
                            Ok(transmute.clone().unwrap())
                        }
                    })*
                }
            }
            fn reset(
                &self, core: &VerifierCore, conn: &DatabaseConnection, guild: Option<GuildId>,
                name: ConfigKeyName,
            ) -> Result<()> {
                match name {
                    $(ConfigKeyName::$name => {
                        reset_db(conn, guild, stringify!($name))?;
                        if guild.is_some() {
                            *self.$name.write() = Some(None);
                        } else {
                            *self.$name.write() = Some(Some($default));
                        }
                    })*
                }
                self.after_update(core, name)
            }
        }

        impl ConfigKeys {
            $(
                #[allow(non_upper_case_globals)]
                pub const $name: ConfigKey<$tp> = ConfigKey {
                    enum_name: ConfigKeyName::$name, _phantom: PhantomData,
                };
            )*
        }
    }
}

config_keys! {
    // Discord settings
    CommandPrefix<String>("!".to_owned());
    DiscordToken<Option<String>>(None, |core| core.discord().reconnect());
    BotOwnerId<Option<u64>>(None);

    // Limits for verification rules
    RolesEnableLimits<bool>(false, |core| Ok(core.roles().clear_rule_cache()));
    RolesMaxAssigned<u32>(15, |core| Ok(core.roles().clear_rule_cache()));
    RolesMaxCustomRules<u32>(30, |core| Ok(core.roles().clear_rule_cache()));
    RolesMaxInstructions<u32>(500, |core| Ok(core.roles().clear_rule_cache()));
    RolesMaxWebRequests<u32>(10, |core| Ok(core.roles().clear_rule_cache()));

    // Role management settings
    SetNickname<bool>(true);

    AllowSetRolesOnJoin<bool>(true);
    SetRolesOnJoin<bool>(true);
    AllowEnableAutoUpdate<bool>(true);
    EnableAutoUpdate<bool>(true);

    MinimumUpdateCooldownSeconds<u64>(60 * 60);
    UpdateCooldownSeconds<u64>(60 * 60);
    MinimumAutoUpdateCooldownSeconds<u64>(60 * 60);
    AutoUpdateCooldownSeconds<u64>(60 * 60 * 24);

    // Verification place settings
    PlaceUITitle<String>("Roblox Account Verifier".to_owned(), |core| core.refresh_place());
    PlaceUIInstructions<String>(
        "To verify your Roblox account with this Discord server, please enter the following \
         command on the server.".to_owned(),
         |core| core.refresh_place()
    );
    PlaceUIBackground<Option<String>>(None, |core| core.refresh_place());
    PlaceID<Option<u64>>(None);

    // Verification settings
    VerificationAttemptLimit<u32>(10);
    VerificationCooldownSeconds<u64>(60 * 60 * 24);

    VerificationChannelIntro<Option<String>>(None);
    VerificationChannelDeleteSeconds<u32>(60);

    TokenValiditySeconds<u32>(60 * 5, |core| {
        core.verifier().rekey(false)?;
        core.refresh_place()?;
        Ok(())
    });

    AllowReverification<bool>(false);
    ReverificationCooldownSeconds<u64>(60 * 60 * 24);
}

struct ConfigManagerData {
    database: Database,
    global_cache: Arc<ConfigCache>,
    guild_cache: ConcurrentCache<GuildId, Arc<ConfigCache>>,
}

#[derive(Clone)]
pub struct ConfigManager(Arc<ConfigManagerData>);
impl ConfigManager {
    pub fn new(database: Database) -> ConfigManager {
        trace!("ConfigCache size: {}", mem::size_of::<ConfigCache>());
        ConfigManager(Arc::new(ConfigManagerData {
            database,
            global_cache: Arc::new(ConfigCache::new()),
            guild_cache: ConcurrentCache::new(|_| Ok(Arc::new(ConfigCache::new()))),
        }))
    }

    fn get_cache(&self, guild: Option<GuildId>) -> Result<Arc<ConfigCache>> {
        match guild {
            Some(guild) =>
                Ok(self.0.guild_cache.read(&guild)?.clone()),
            None =>
                Ok(self.0.global_cache.clone()),
        }
    }
    pub fn set<T : ToSql + Clone + Any + Send + Sync>(
        &self, core: &VerifierCore, guild: Option<GuildId>, key: ConfigKey<T>, val: T,
    ) -> Result<()> {
        self.get_cache(guild)?.set(core, &self.0.database.connect()?, guild, key, val)
    }
    pub fn reset<T: Clone + Any + Send + Sync>(
        &self, core: &VerifierCore, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<()> {
        self.get_cache(guild)?.reset(core, &self.0.database.connect()?, guild, key.enum_name)
    }

    fn get_internal<T : ToSql + FromSql + Clone + Any + Send + Sync>(
        &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        let result = self.get_cache(guild)?.get(&self.0.database.connect()?, guild, key)?;
        if guild.is_some() {
            match result {
                Some(res) => Ok(res),
                None => self.get_internal(conn, None, key),
            }
        } else {
            Ok(result.unwrap())
        }
    }
    pub fn get<T : ToSql + FromSql + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        self.get_internal(&self.0.database.connect()?, guild, key)
    }

    pub fn on_cleanup_tick(&self) {
        self.0.guild_cache.shrink_to_fit();
    }
    pub fn on_guild_remove(&self, guild: GuildId) {
        self.0.guild_cache.remove(&guild);
    }
}