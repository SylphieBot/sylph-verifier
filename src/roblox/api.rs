use errors::*;
use percent_encoding::{percent_encode, QUERY_ENCODE_SET};
use reqwest;
use roblox::*;
use serde_json;
use std::collections::{HashSet, HashMap};

#[derive(Deserialize)]
struct RobloxIDLookup {
    #[serde(rename = "Id")] id: u64,
    #[serde(rename = "Username")] name: String,
}

#[derive(Deserialize)]
struct RobloxDevForumUserLookup {
    trust_level: u32,
}

#[derive(Deserialize)]
struct RobloxDevForumLookup {
    user: RobloxDevForumUserLookup,
}

#[derive(Deserialize)]
struct RobloxBadgeLookup {
    #[serde(rename = "Name")] name: String,
}

#[derive(Deserialize)]
struct RobloxBadgesLookup {
    #[serde(rename = "RobloxBadges")] badges: Vec<RobloxBadgeLookup>,
}

#[derive(Deserialize)]
struct RobloxGroupLookup {
    #[serde(rename = "Id")] id: u64,
    #[serde(rename = "Rank")] rank: u32,
}

pub fn for_username(name: &str) -> Result<RobloxUserID> {
    let uri = format!("https://api.roblox.com/users/get-by-username?username={}",
                      percent_encode(name.as_bytes(), QUERY_ENCODE_SET));
    let json = reqwest::get(&uri)?.error_for_status()?.text()?;
    let info = serde_json::from_str::<RobloxIDLookup>(&json)?;
    Ok(RobloxUserID(info.id))
}

pub fn lookup_username(id: RobloxUserID) -> Result<String> {
    let uri = format!("https://api.roblox.com/users/{}", id.0);
    let json = reqwest::get(&uri)?.error_for_status()?.text()?;
    let info = serde_json::from_str::<RobloxIDLookup>(&json)?;
    Ok(info.name)
}

pub fn get_dev_trust_level(name: &str) -> Result<Option<u32>> {
    let uri = format!("https://devforum.roblox.com/users/{}.json",
                      percent_encode(name.as_bytes(), QUERY_ENCODE_SET));
    let mut request = reqwest::get(&uri)?;
    if request.status().is_success() {
        let lookup = serde_json::from_str::<RobloxDevForumLookup>(&request.text()?)?;
        Ok(Some(lookup.user.trust_level))
    } else {
        Ok(None)
    }
}

pub fn owns_asset(id: RobloxUserID, asset: u64) -> Result<bool> {
    let uri = format!("https://api.roblox.com/Ownership/HasAsset?userId={}&assetId={}",
                      id.0, asset);
    let text = reqwest::get(&uri)?.error_for_status()?.text()?;
    Ok(text == "true")
}

pub fn get_roblox_badges(id: RobloxUserID) -> Result<HashSet<String>> {
    let uri = format!("https://www.roblox.com/badges/roblox?userId={}", id.0);
    let json = reqwest::get(&uri)?.error_for_status()?.text()?;
    let badges = serde_json::from_str::<RobloxBadgesLookup>(&json)?;
    Ok(badges.badges.into_iter().map(|x| x.name).collect())
}

pub fn has_player_badge(id: RobloxUserID, asset: u64) -> Result<bool> {
    let uri = format!("https://assetgame.roblox.com/Game/Badge/HasBadge.ashx?UserID={}&BadgeID={}",
                      id.0, asset);
    Ok(reqwest::get(&uri)?.error_for_status()?.text()? == "Success")
}

pub fn get_player_groups(id: RobloxUserID) -> Result<HashMap<u64, u32>> {
    let uri = format!("https://api.roblox.com/users/{}/groups", id.0);
    let json = reqwest::get(&uri)?.error_for_status()?.text()?;
    let groups = serde_json::from_str::<Vec<RobloxGroupLookup>>(&json)?;
    let mut map = HashMap::new();
    for RobloxGroupLookup { id, rank } in groups {
        map.insert(id, rank);
    }
    Ok(map)
}
