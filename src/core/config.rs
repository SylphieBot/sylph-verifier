use core::database::*;
use core::database::schema::*;
use diesel;
use diesel::prelude::*;
use errors::*;
use parking_lot::RwLock;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json;
use serenity::model::GuildId;
use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

// TODO: Get rid of Clone here?
// TODO: Prevent per-guild cache from growing indefinitely.

struct ConfigKeyData<T> {
    // Storage related
    db_name: &'static str, default: fn() -> T, _phantom: PhantomData<(fn(T), fn() -> T)>,
}

pub struct ConfigKey<T: 'static>(&'static ConfigKeyData<T>);
impl <T> Clone for ConfigKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl <T> Copy for ConfigKey<T> { }

pub enum ConfigKeys { }

macro_rules! config_keys {
    ($($name:ident<$tp:ty>($default:expr);)*) => {
        $(
            #[allow(non_upper_case_globals)]
            const $name: &'static ConfigKeyData<$tp> = &ConfigKeyData {
                db_name: stringify!($name), _phantom: PhantomData, default: || $default,
            };
        )*

        impl ConfigKeys {
            $(
                #[allow(non_upper_case_globals)]
                pub const $name: ConfigKey<$tp> = ConfigKey($name);
            )*
        }

        const ALL_KEYS: &'static [&'static str] = &[
            $(ConfigKeys::$name.0.db_name,)*
        ];
    }
}

config_keys! {
    // Discord settings
    CommandPrefix<String>("!".to_owned());
    DiscordToken<Option<String>>(None);

    // Verification settings
    VerificationAttemptLimit<u32>(10);
    VerificationCooldownSeconds<u64>(60 * 60 * 24);

    TokenValiditySeconds<i32>(60 * 5);

    AllowReverification<bool>(true);
    ReverificationTimeoutSeconds<u64>(0);
}

type ConfigValue = Option<Box<Any + Send + Sync + 'static>>;
type ValueContainer = RwLock<ConfigValue>;
struct ConfigCache(HashMap<&'static str, ValueContainer>);
impl ConfigCache {
    fn new() -> ConfigCache {
        let mut cache = HashMap::new();
        for &name in ALL_KEYS {
            cache.insert(name, RwLock::new(None));
        }
        ConfigCache(cache)
    }

    fn get<T>(&self, key: ConfigKey<T>) -> &ValueContainer {
        self.0.get(&key.0.db_name).unwrap()
    }
}

struct ConfigManagerData {
    database: Database,
    global_cache: ConfigCache, guild_cache: RwLock<HashMap<GuildId, ConfigCache>>,
}

#[derive(Clone)]
pub struct ConfigManager(Arc<ConfigManagerData>);
impl ConfigManager {
    pub fn new(database: Database) -> ConfigManager {
        ConfigManager(Arc::new(ConfigManagerData {
            database, global_cache: ConfigCache::new(), guild_cache: RwLock::new(HashMap::new()),
        }))
    }

    fn get_db(&self, conn: &DatabaseConnection, guild: Option<GuildId>,
              key: &str) -> Result<Option<String>> {
        Ok(match guild {
            Some(guild) =>
                 guild_config::table
                    .filter(guild_config::discord_guild_id.eq(guild.0 as i64)
                        .or(guild_config::key             .eq(key)))
                    .select(guild_config::value)
                    .get_result(conn.deref()).optional()?,
            None =>
                 global_config::table
                    .filter(global_config::key.eq(key))
                    .select(global_config::value)
                    .get_result(conn.deref()).optional()?,
        })
    }
    fn set_db(&self, conn: &DatabaseConnection, guild: Option<GuildId>,
              key: &str, value: &str) -> Result<()> {
        match guild {
            Some(guild) => {
                diesel::replace_into(guild_config::table).values((
                    guild_config::discord_guild_id.eq(guild.0 as i64),
                    guild_config::key             .eq(key),
                    guild_config::value           .eq(value),
                )).execute(conn.deref())?;
            }
            None => {
                diesel::replace_into(global_config::table).values((
                    global_config::key  .eq(key),
                    global_config::value.eq(value),
                )).execute(conn.deref())?;
            }
        }
        Ok(())
    }
    fn reset_db(&self, conn: &DatabaseConnection, guild: Option<GuildId>,
                key: &str) -> Result<()> {
        match guild {
            Some(guild) => {
                diesel::delete(
                    guild_config::table
                        .filter(guild_config::discord_guild_id.eq(guild.0 as i64)
                           .and(guild_config::key             .eq(key)))
                ).execute(conn.deref())?;
            }
            None => {
                diesel::delete(
                    global_config::table.filter(global_config::key.eq(key))
                ).execute(conn.deref())?;
            }
        }
        Ok(())
    }

    fn get_cache<T, F>(
        &self, guild: Option<GuildId>, f: F
    ) -> T where F: FnOnce(&ConfigCache) -> T {
        match guild {
            Some(guild) => {
                if self.0.guild_cache.read().get(&guild).is_some() {
                    return f(self.0.guild_cache.read().get(&guild).unwrap())
                }

                // None case
                let mut guild_cache = self.0.guild_cache.write();
                if guild_cache.get(&guild).is_none() {
                    guild_cache.insert(guild, ConfigCache::new());
                }
                f(guild_cache.get(&guild).unwrap())
            }
            None => f(&self.0.global_cache),
        }
    }

    pub fn set<T : Serialize + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>, val: T,
    ) -> Result<()> {
        self.get_cache(guild, |cache| {
            let conn = self.0.database.connect()?;
            let mut cache = cache.get(key).write();
            self.set_db(&conn, guild, key.0.db_name, &serde_json::to_string(&val)?)?;
            *cache = Some(Box::new(val.clone()));
            Ok(())
        })
    }
    pub fn reset<T: Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<()> {
        self.get_cache(guild, |cache| {
            let conn = self.0.database.connect()?;
            let mut cache = cache.get(key).write();
            self.reset_db(&conn, guild, key.0.db_name)?;
            let val = (key.0.default)();
            *cache = Some(Box::new(val.clone()));
            Ok(())
        })
    }

    fn get_internal<T : Serialize + DeserializeOwned + Clone + Any + Send + Sync>(
        &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        self.get_cache(guild, |cache| {
            let lock = cache.get(key);

            if let &Some(ref any) = &*lock.read() {
                return Ok(Any::downcast_ref::<T>(any.deref()).unwrap().clone())
            }

            // None case
            match guild {
                Some(_) => self.get_internal(conn, None, key),
                None => {
                    let mut cache = lock.write();
                    if cache.is_some() {
                        let any = cache.as_mut().unwrap();
                        Ok(Any::downcast_ref::<T>(any.deref()).unwrap().clone())
                    } else {
                        conn.transaction_immediate(|| {
                            let value = match self.get_db(conn, None, key.0.db_name)? {
                                Some(value) => serde_json::from_str::<T>(&value)?,
                                None => (key.0.default)(),
                            };
                            *cache = Some(Box::new(value.clone()));
                            Ok(value)
                        })
                    }
                }
            }
        })
    }
    pub fn get<T : Serialize + DeserializeOwned + Clone + Any + Send + Sync>(
        &self, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        self.get_internal(&self.0.database.connect()?, guild, key)
    }
}