use core::*;
use core::schema::*;
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

// TODO: Get rid of Clone here?
// TODO: Prevent per-guild cache from growing indefinitely.

pub struct ConfigKey<T> {
    db_name: &'static str, default: fn() -> T,
    _phantom: PhantomData<(fn(T), fn() -> T)>,
}
impl <T> Clone for ConfigKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl <T> Copy for ConfigKey<T> { }

pub enum ConfigKeys { }

macro_rules! config_keys {
    ($($name:ident: $tp:ty => $default:expr;)*) => {
        $(
            impl ConfigKeys {
                #[allow(non_upper_case_globals)]
                pub const $name: ConfigKey<$tp> = ConfigKey {
                    db_name: stringify!($name), _phantom: PhantomData, default: || $default,
                };
            }
        )*

        const ALL_KEYS: &'static [&'static str] = &[
            $(ConfigKeys::$name.db_name,)*
        ];
    }
}

config_keys! {
    GlobalCommandPrefix: String => "!".to_owned();
}

type ValueContainer = RwLock<Option<Box<Any + Send + Sync + 'static>>>;
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
        self.0.get(&key.db_name).unwrap()
    }
}

pub struct ConfigManager {
    global_cache: ConfigCache, guild_cache: RwLock<HashMap<GuildId, ConfigCache>>,
}
impl ConfigManager {
    pub fn new() -> ConfigManager {
        ConfigManager {
            global_cache: ConfigCache::new(), guild_cache: RwLock::new(HashMap::new()),
        }
    }

    fn get_db(&self, conn: &DatabaseConnection, guild: Option<GuildId>,
              key: &str) -> Result<Option<String>> {
        Ok(match guild {
            Some(guild) =>
                 guild_config::table
                    .filter(guild_config::discord_guild_id.eq(guild.0 as i64)
                        .or(guild_config::key             .eq(key)))
                    .select(guild_config::value)
                    .load::<String>(conn.deref())?.into_iter().next(),
            None =>
                 global_config::table
                    .filter(global_config::key.eq(key))
                    .select(global_config::value)
                    .load::<String>(conn.deref())?.into_iter().next(),
        })
    }
    fn set_db(&self, conn: &DatabaseConnection, guild: Option<GuildId>,
              key: &str, value: &str) -> Result<()> {
        match guild {
            Some(guild) => {
                ::diesel::replace_into(guild_config::table).values((
                    guild_config::discord_guild_id.eq(guild.0 as i64),
                    guild_config::key             .eq(key),
                    guild_config::value           .eq(value),
                )).execute(conn.deref())?;
            }
            None => {
                ::diesel::replace_into(global_config::table).values((
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
                ::diesel::delete(
                    guild_config::table
                        .filter(guild_config::discord_guild_id.eq(guild.0 as i64)
                           .and(guild_config::key             .eq(key)))
                ).execute(conn.deref())?;
            }
            None => {
                ::diesel::delete(
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
                if let Some(cache) = self.guild_cache.read().get(&guild) {
                    f(cache)
                } else {
                    let mut guild_cache = self.guild_cache.write();
                    if guild_cache.get(&guild).is_none() {
                        guild_cache.insert(guild, ConfigCache::new());
                    }
                    f(guild_cache.get(&guild).unwrap())
                }
            }
            None => f(&self.global_cache),
        }
    }

    pub fn set<T : Serialize + Any + Send + Sync + 'static>(
        &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>, val: T
    ) -> Result<()> {
        self.get_cache(guild, |cache| {
            let mut cache = cache.get(key).write();
            self.set_db(conn, guild, key.db_name, &serde_json::to_string(&val)?);
            *cache = Some(Box::new(val));
            Ok(())
        })
    }
    pub fn reset<T: Any + Send + Sync + 'static>(
        &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<()> {
        self.get_cache(guild, |cache| {
            let mut cache = cache.get(key).write();
            self.reset_db(conn, guild, key.db_name);
            *cache = Some(Box::new((key.default)()));
            Ok(())
        })
    }
    pub fn get<T : Serialize + DeserializeOwned + Clone + Any + Send + Sync + 'static>(
        &self, conn: &DatabaseConnection, guild: Option<GuildId>, key: ConfigKey<T>
    ) -> Result<T> {
        self.get_cache(guild, |cache| {
            let lock = cache.get(key);
            if let &Some(ref any) = &*lock.read() {
                Ok(Any::downcast_ref::<T>(any.deref()).unwrap().clone())
            } else {
                match guild {
                    Some(_) => self.get(conn, None, key),
                    None => {
                        let mut cache = lock.write();
                        if cache.is_some() {
                            let any = cache.as_mut().unwrap();
                            Ok(Any::downcast_ref::<T>(any.deref()).unwrap().clone())
                        } else {
                            conn.transaction_immediate(|| {
                                match self.get_db(conn, None, key.db_name)? {
                                    Some(value) => {
                                        let value = serde_json::from_str::<T>(&value)?;
                                        *cache = Some(Box::new(value.clone()));
                                        Ok(value)
                                    }
                                    None => {
                                        let value = (key.default)();
                                        self.set(conn, None, key, value.clone())?;
                                        Ok(value)
                                    }
                                }
                            })
                        }
                    }
                }
            }
        })
    }
}