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

// TODO: Get rid of Clone here?
// TODO: Prevent per-guild cache from growing indefinitely.
// TODO: Consider minimizing ConfigCache size (probably not needed)

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
    ($($name:ident<$tp:ty>($default:expr);)*) => {
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

            fn set<T: ToSql>(
                &self, conn: &DatabaseConnection, guild: Option<GuildId>,
                key: ConfigKey<T>, value: T,
            ) -> Result<()> {
                match key.enum_name {
                    $(ConfigKeyName::$name => {
                        if TypeId::of::<T>() != TypeId::of::<$tp>() {
                            get_db_type_panic()
                        } else {
                            set_db(conn, guild, stringify!($name), &value)?;

                            // This will only execute if transmute is an noop, so this is safe.
                            let mut write_ptr = self.$name.write();
                            let mut transmute: &mut Option<Option<T>> = unsafe {
                                mem::transmute(write_ptr.deref_mut())
                            };
                            *transmute = Some(Some(value));
                        }
                    })*
                }
                Ok(())
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
                &self, conn: &DatabaseConnection, guild: Option<GuildId>, name: ConfigKeyName,
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
                Ok(())
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
    DiscordToken<Option<String>>(None);

    // Limits for verification rules
    RolesEnableLimits<bool>(false);
    RolesMaxAssigned<u32>(15);
    RolesMaxActiveCustomRules<u32>(15);
    RolesMaxInstructions<u32>(500);
    RolesMaxWebRequests<u32>(10);

    // Role management settings
    SetUsername<bool>(true);
    UpdateCooldownSeconds<u64>(60 * 60);

    SetRolesOnJoin<bool>(false);

    EnableAutoUpdate<bool>(false);
    AutoUpdateCooldownSeconds<u64>(60 * 60 * 24);

    // Verification place settings
    PlaceUITitle<String>("Roblox Account Verifier".to_owned());
    PlaceUIInstructions<String>(
        "To verify your Roblox account with this Discord server, please enter the following \
         command on the server.".to_owned()
    );
    PlaceUIBackground<Option<String>>(None);
    PlaceID<Option<u64>>(None);

    // Verification settings
    VerificationAttemptLimit<u32>(10);
    VerificationCooldownSeconds<u64>(60 * 60 * 24);

    TokenValiditySeconds<u32>(60 * 5);

    AllowReverification<bool>(false);
    ReverificationCooldownSeconds<u64>(60 * 60 * 24);
}

struct ConfigManagerData {
    database: Database,
    global_cache: ConfigCache, guild_cache: ConcurrentCache<GuildId, ConfigCache>,
}

#[derive(Clone)]
pub struct ConfigManager(Arc<ConfigManagerData>);
impl ConfigManager {
    pub fn new(database: Database) -> ConfigManager {
        ConfigManager(Arc::new(ConfigManagerData {
            database, global_cache: ConfigCache::new(), guild_cache: ConcurrentCache::new(),
        }))
    }

    fn get_cache<T, F>(
        &self, guild: Option<GuildId>, f: F
    ) -> Result<T> where F: FnOnce(&ConfigCache) -> Result<T> {
        match guild {
            Some(guild) => self.0.guild_cache.get_cached(&guild, || Ok(ConfigCache::new()), f),
            None => f(&self.0.global_cache),
        }
    }

    pub fn set<T : ToSql + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>, val: T,
    ) -> Result<()> {
        self.get_cache(guild, |cache| {
            cache.set(&self.0.database.connect()?, guild, key, val)
        })
    }
    pub fn reset<T: Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<()> {
        self.get_cache(guild, |cache| {
            cache.reset(&self.0.database.connect()?, guild, key.enum_name)
        })
    }

    fn get_internal<T : ToSql + FromSql + Clone + Any + Send + Sync>(
        &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        let result =
            self.get_cache(guild, |cache| cache.get(&self.0.database.connect()?, guild, key))?;
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
}