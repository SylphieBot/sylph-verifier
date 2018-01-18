use errors::*;
use parking_lot::{RwLock, RwLockReadGuard};
use reqwest;
use serenity::model::prelude::*;
use std::borrow::Borrow;
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

// Time to i64
pub fn time_from_i64(time: i64) -> SystemTime {
    assert_ne!(time, i64::min_value());
    if time >= 0 {
        UNIX_EPOCH + Duration::from_secs(time as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs((-time) as u64)
    }
}
fn check_u64_to_i64(val: u64, is_neg: bool) -> i64 {
    assert!(val < i64::max_value() as u64);
    if is_neg {
        -(val as i64)
    } else {
        val as i64
    }
}
pub fn time_to_i64(time: SystemTime) -> i64 {
    if time >= UNIX_EPOCH {
        check_u64_to_i64(time.duration_since(UNIX_EPOCH).unwrap().as_secs(), false)
    } else {
        check_u64_to_i64(time.duration_since(UNIX_EPOCH).unwrap().as_secs(), true)
    }
}

// Atomic time
pub struct AtomicSystemTime(AtomicI64);
impl AtomicSystemTime {
    pub fn new(time: Option<SystemTime>) -> AtomicSystemTime {
        AtomicSystemTime(AtomicI64::new(time.map_or(i64::min_value(), time_to_i64)))
    }
    pub fn store(&self, time: Option<SystemTime>) {
        self.0.store(time.map_or(i64::min_value(), time_to_i64), Ordering::Relaxed)
    }
    pub fn load(&self) -> Option<SystemTime> {
        let val = self.0.load(Ordering::Relaxed);
        if val == i64::min_value() { None } else { Some(time_from_i64(val)) }
    }
}

// Concurrent cache implementation
pub struct ConcurrentCache<K: Clone + Eq + Hash + Sync, V: Sync>(RwLock<HashMap<K, V>>);
impl <K: Clone + Eq + Hash + Sync, V: Sync> ConcurrentCache<K, V> {
    pub fn new() -> Self {
        ConcurrentCache(RwLock::new(HashMap::new()))
    }

    pub fn read<F>(
        &self, k: &K, create: F
    ) -> Result<RwLockReadGuard<V>> where F: FnOnce() -> Result<V> {
        {
            let read = self.0.read();
            if read.contains_key(k) {
                return Ok(RwLockReadGuard::map(read, |x| x.get(k).unwrap()))
            }
        }

        {
            let new_value = create()?;
            let mut write = self.0.write();
            if !write.contains_key(&k) {
                write.insert(k.clone(), new_value);
            }
        }

        Ok(RwLockReadGuard::map(self.0.read(), |x| x.get(k).unwrap()))
    }

    pub fn remove<Q: Eq + Hash>(&self, k: &Q) -> Option<V> where K: Borrow<Q> {
        self.0.write().remove(k)
    }
    pub fn retain<F>(&self, mut f: F) where F: FnMut(&K, &V) -> bool {
        self.0.write().retain(|k, v| f(k, &v));
    }
    pub fn clear_cache(&self) {
        self.0.write().clear()
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