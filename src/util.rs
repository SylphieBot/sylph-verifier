use errors::*;
use parking_lot::RwLock;
use reqwest;
use serenity::model::prelude::*;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn div_ceil(a: u64, b: u64) -> u64 {
    if a == 0 {
        0
    } else {
        (a - 1) / b + 1
    }
}
fn english_pluralize(i: u64, singular: &str, plural: &str) -> String {
    if i == 1 {
        format!("1 {}", singular)
    } else {
        format!("{} {}", i, plural)
    }
}
pub fn to_english_time(secs: u64) -> String {
    if secs < 60 {
        english_pluralize(secs, "second", "seconds")
    } else if secs < 60 * 60 {
        english_pluralize(div_ceil(secs, 60), "minute", "minutes")
    } else {
        english_pluralize(div_ceil(secs, 60 * 60), "hour", "hours")
    }
}
pub fn to_english_time_precise(secs: u64) -> String {
    if secs < 60 {
        english_pluralize(secs, "second", "seconds")
    } else {
        format!("{} = {}", english_pluralize(secs, "second", "seconds"), to_english_time(secs))
    }
}
pub fn english_time_diff(from: SystemTime, to: SystemTime) -> String {
    to_english_time(to.duration_since(from).map(|x| x.as_secs()).unwrap_or(0))
}

// Concurrent cache implementation
struct CacheValue<V: Sync>(V, AtomicI64);
pub struct ConcurrentCache<K: Clone + Eq + Hash + Sync, V: Sync>(RwLock<HashMap<K, CacheValue<V>>>);
impl <K: Clone + Eq + Hash + Sync, V: Sync> ConcurrentCache<K, V> {
    pub fn new() -> Self {
        ConcurrentCache(RwLock::new(HashMap::new()))
    }

    pub fn get_cached<F, G, R>(
        &self, k: &K, create: F, closure: G
    ) -> Result<R> where F: FnOnce() -> Result<V>, G: FnOnce(&V) -> Result<R> {
        {
            let read = self.0.read();
            if let Some(value) = read.get(k) {
                value.1.store(time_to_i64(SystemTime::now())?, Ordering::Relaxed);
                return closure(&value.0)
            }
        }

        // None case
        let mut write = self.0.write();
        if write.get(k).is_none() {
            let new = CacheValue(create()?, AtomicI64::new(time_to_i64(SystemTime::now())?));
            write.insert(k.clone(), new);
        }
        closure(&write[k].0)
    }

    pub fn retain<F>(&self, mut f: F) where F: FnMut(&K, &mut V) -> bool {
        self.0.write().retain(|k, v| f(k, &mut v.0));
    }
    pub fn clear_old(&self, cutoff_duration: Duration) {
        let cutoff = SystemTime::now() - cutoff_duration;
        self.0.write().retain(|_, v|
            time_from_i64(v.1.load(Ordering::Relaxed)).unwrap() >= cutoff
        );
    }
    pub fn clear_cache(&self) {
        self.0.write().clear()
    }
}

// Time to i64
pub fn time_from_i64(time: i64) -> Result<SystemTime> {
    ensure!(time != i64::min_value(), "time must not be i64::min_value()");
    if time >= 0 {
        Ok(UNIX_EPOCH + Duration::from_secs(time as u64))
    } else {
        Ok(UNIX_EPOCH - Duration::from_secs((-time) as u64))
    }
}
fn check_u64_to_i64(val: u64, is_neg: bool) -> Result<i64> {
    ensure!(val < i64::max_value() as u64, "time must fit in an i64");
    if is_neg {
        Ok(-(val as i64))
    } else {
        Ok(val as i64)
    }
}
pub fn time_to_i64(time: SystemTime) -> Result<i64> {
    if time >= UNIX_EPOCH {
        check_u64_to_i64(time.duration_since(UNIX_EPOCH)?.as_secs(), false)
    } else {
        check_u64_to_i64(time.duration_since(UNIX_EPOCH)?.as_secs(), true)
    }
}

// Command IDs
static COMMAND_ID: AtomicUsize = AtomicUsize::new(0);
pub fn command_id() -> usize {
    COMMAND_ID.fetch_add(1, Ordering::Relaxed)
}

// Pasting text
pub fn sprunge(text: &str) -> Result<String> {
    let mut params = HashMap::new();
    params.insert("sprunge", text);

    let client = reqwest::Client::new();
    let mut result = client.post("http://sprunge.us/").form(&params).send()?.error_for_status()?;
    Ok(result.text()?.trim().to_string())
}

// TODO: Wait for these to be added to Serenity.
#[derive(Ord, PartialOrd, Eq, PartialEq)]
enum RolePosition {
    Nobody, Role(i64), GuildOwner,
}
fn can_access(from: RolePosition, to: RolePosition) -> bool {
    if from == RolePosition::GuildOwner {
        true
    } else {
        from > to
    }
}
fn member_position(member: &Member) -> Result<RolePosition> {
    let owner_id = member.guild_id.find().chain_err(|| "Could not get guild.")?;
    let owner_id = owner_id.read().owner_id;
    if member.user.read().id == owner_id {
        Ok(RolePosition::GuildOwner)
    } else {
        let roles = member.roles().chain_err(|| "Could not get roles.")?;
        if roles.is_empty() {
            Ok(RolePosition::Nobody)
        } else {
            Ok(RolePosition::Role(roles.iter().map(|x| x.position).max().unwrap()))
        }
    }
}
pub fn can_member_access_role(member: &Member, role: RoleId) -> Result<bool> {
    let role_position =
        RolePosition::Role(role.find().chain_err(|| "Could not get role.")?.position);
    Ok(can_access(member_position(member)?, role_position))
}
pub fn can_member_access_member(from: &Member, to: &Member) -> Result<bool> {
    Ok(can_access(member_position(from)?, member_position(to)?))
}