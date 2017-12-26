use errors::*;
use parking_lot::RwLock;
use reqwest;
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::SystemTime;

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
pub fn english_time_diff(from: SystemTime, to: SystemTime) -> String {
    to_english_time(to.duration_since(from).map(|x| x.as_secs()).unwrap_or(0))
}

pub struct ConcurrentCache<K: Clone + Eq + Hash + Sync, V: Sync>(RwLock<HashMap<K, V>>);
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
                return closure(value)
            }
        }

        // None case
        let mut write = self.0.write();
        if write.get(k).is_none() {
            write.insert(k.clone(), create()?);
        }
        closure(&write[k])
    }
}

pub fn sprunge(text: &str) -> Result<String> {
    let mut params = HashMap::new();
    params.insert("sprunge", text);

    let client = reqwest::Client::new();
    let mut result = client.post("http://sprunge.us/").form(&params).send()?.error_for_status()?;
    Ok(result.text()?.trim().to_string())
}
pub fn cmd_sprunge<'a>(text: &'a str) -> Result<Cow<'a, str>> {
    if text.len() > 1900 {
        Ok(sprunge(text)?.into())
    } else {
        Ok(text.into())
    }
}