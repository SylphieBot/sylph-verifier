use errors::*;
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use reqwest;
use serenity::model::prelude::*;
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::mem::drop;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

// MultiMutex implementation
pub struct MultiMutexGuard<T: Hash + Eq>(Arc<Mutex<HashSet<T>>>, T);
impl <T: Hash + Eq> Drop for MultiMutexGuard<T> {
    fn drop(&mut self) {
        self.0.lock().remove(&self.1);
    }
}

#[derive(Clone)]
pub struct MultiMutex<T: Hash + Eq>(Arc<Mutex<HashSet<T>>>);
impl <T: Hash + Eq + Clone> MultiMutex<T> {
    pub fn new() -> MultiMutex<T> {
        MultiMutex(Arc::new(Mutex::new(HashSet::new())))
    }

    pub fn lock(&self, id: T) -> Option<MultiMutexGuard<T>> {
        let mut lock = self.0.lock();
        if lock.contains(&id) {
            None
        } else {
            lock.insert(id.clone());
            Some(MultiMutexGuard(self.0.clone(), id))
        }
    }

    pub fn shrink_to_fit(&self) {
        self.0.lock().shrink_to_fit();
    }
}

// Concurrent cache implementation
// TODO: Add *optional* support for tracking last access in this.
pub struct ConcurrentCache<K: Clone + Eq + Hash + Sync, V: Sync> {
    data: RwLock<HashMap<K, V>>, create: Box<Fn(&K) -> Result<V> + Send + Sync + 'static>,
}
impl <K: Clone + Eq + Hash + Sync, V: Sync> ConcurrentCache<K, V> {
    pub fn new<F>(f: F) -> Self where F: Fn(&K) -> Result<V> + Send + Sync + 'static {
        ConcurrentCache {
            data: RwLock::new(HashMap::new()), create: Box::new(f),
        }
    }

    pub fn read(&self, k: &K) -> Result<RwLockReadGuard<V>> {
        loop {
            let read = self.data.read();
            if read.contains_key(k) {
                return Ok(RwLockReadGuard::map(read, |x| x.get(k).unwrap()))
            }
            drop(read);

            let new_value = (self.create)(k)?;
            let mut write = self.data.write();
            if !write.contains_key(&k) {
                write.insert(k.clone(), new_value);
            }
        }
    }
    pub fn write(&self, k: &K) -> Result<RwLockWriteGuard<V>> {
        let write = self.data.write();
        if write.contains_key(k) {
            Ok(RwLockWriteGuard::map(write, |x| x.get_mut(k).unwrap()))
        } else {
            drop(write);

            let new_value = (self.create)(k)?;
            let mut write = self.data.write();
            if !write.contains_key(&k) {
                write.insert(k.clone(), new_value);
            }
            Ok(RwLockWriteGuard::map(write, |x| x.get_mut(k).unwrap()))
        }
    }

    pub fn for_each<F>(&self, mut f: F) where F: FnMut(&V) {
        for (_, v) in self.data.read().iter() {
            f(v);
        }
    }
    pub fn shrink_to_fit(&self) {
        self.data.write().shrink_to_fit();
    }
    pub fn remove<Q: Eq + Hash>(&self, k: &Q) -> Option<V> where K: Borrow<Q> {
        self.data.write().remove(k)
    }
    pub fn retain<F>(&self, mut f: F) where F: FnMut(&K, &V) -> bool {
        self.data.write().retain(|k, v| f(k, &v));
    }
    pub fn clear_cache(&self) {
        *self.data.write() = HashMap::new();
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

// Hierarchy access helpers
pub fn can_member_access_role(guild_id: GuildId, member_id: UserId, role: RoleId) -> Result<bool> {
    let guild = guild_id.find()?;
    let owner_id = guild.read().owner_id;

    if member_id == owner_id {
        Ok(true)
    } else {
        match guild.read().member(member_id)?.highest_role_info() {
            Some((_, position)) => Ok(role.find()?.position < position),
            None => Ok(false),
        }
    }
}
pub fn can_member_access_member(guild_id: GuildId, from: UserId, to: UserId) -> Result<bool> {
    let guild = guild_id.find()?;
    Ok(from == to || guild.read().greater_member_hierarchy(from, to) == Some(from))
}