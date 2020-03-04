use errors::*;
use percent_encoding::{percent_encode, QUERY_ENCODE_SET};
use reqwest;
use reqwest::header;
use reqwest::StatusCode;
use roblox::*;
use scraper::{Html, Selector};
use serde_json;
use std::collections::{HashSet, HashMap};

#[derive(Deserialize)]
struct RobloxIDLookup {
    #[serde(rename = "Id")] id: Option<u64>,
    #[serde(rename = "Username")] name: Option<String>,
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

crate struct WebProfileInfo {
    crate profile_exists: bool,
    crate has_premium: bool,
}

fn get_api_endpoint(uri: &str) -> Result<reqwest::Response> {
    let mut headers = header::HeaderMap::new();
    let agent = concat!(
        "SylphVerifierBot/", env!("CARGO_PKG_VERSION"), " (+https://github.com/SylphieBot/sylph-verifier)"
    );
    headers.insert(header::USER_AGENT, header::HeaderValue::from_static(agent));
    let client = reqwest::Client::builder().default_headers(headers).build()?;
    Ok(client.get(uri).send()?)
}

crate fn get_web_profile(id: RobloxUserID) -> Result<WebProfileInfo> {
    let uri = format!("https://www.roblox.com/users/{}/profile", id.0);
    let response = get_api_endpoint(&uri)?;
    let mut response = if response.status() != StatusCode::NOT_FOUND {
        response.error_for_status()?
    } else {
        response
    };

    let mut info = WebProfileInfo {
        profile_exists: false,
        has_premium: false
    };
    if response.url().as_str() == "https://www.roblox.com/request-error?code=404" {
        info.profile_exists = true;
    } else {
        let document = Html::parse_document(&response.text()?);
        lazy_static! {
            static ref SELECTOR_PREMIUM_MEDIUM: Selector =
                Selector::parse(".icon-premium-medium").unwrap();
            static ref SELECTOR_PREMIUM_SMALL: Selector =
                Selector::parse(".icon-premium-small").unwrap();
        }

        let has_medium = document.select(&*SELECTOR_PREMIUM_MEDIUM).next().is_some();
        let has_small = document.select(&*SELECTOR_PREMIUM_SMALL).next().is_some();
        if has_medium || has_small {
            info.has_premium = true;
        }

    }
    Ok(info)
}

crate fn for_username(name: &str) -> Result<Option<RobloxUserID>> {
    let uri = format!("https://api.roblox.com/users/get-by-username?username={}",
                      percent_encode(name.as_bytes(), QUERY_ENCODE_SET));
    let json = get_api_endpoint(&uri)?.error_for_status()?.text()?;
    let info = serde_json::from_str::<RobloxIDLookup>(&json)?;
    Ok(info.id.map(RobloxUserID))
}

crate fn lookup_username(id: RobloxUserID) -> Result<Option<String>> {
    let uri = format!("https://api.roblox.com/users/{}", id.0);
    let json = get_api_endpoint(&uri)?.error_for_status()?.text()?;
    let info = serde_json::from_str::<RobloxIDLookup>(&json)?;
    Ok(info.name)
}

crate fn get_dev_trust_level(name: &str) -> Result<Option<u32>> {
    let uri = format!("https://devforum.roblox.com/users/{}.json",
                      percent_encode(name.as_bytes(), QUERY_ENCODE_SET));
    let mut request = get_api_endpoint(&uri)?;
    if request.status().is_success() {
        let lookup = serde_json::from_str::<RobloxDevForumLookup>(&request.text()?)?;
        Ok(Some(lookup.user.trust_level))
    } else {
        Ok(None)
    }
}

crate fn owns_asset(id: RobloxUserID, asset: u64) -> Result<bool> {
    let uri = format!("https://api.roblox.com/Ownership/HasAsset?userId={}&assetId={}",
                      id.0, asset);
    let text = get_api_endpoint(&uri)?.error_for_status()?.text()?;
    Ok(text == "true")
}

crate fn get_roblox_badges(id: RobloxUserID) -> Result<HashSet<String>> {
    let uri = format!("https://www.roblox.com/badges/roblox?userId={}", id.0);
    let json = get_api_endpoint(&uri)?.error_for_status()?.text()?;
    let badges = serde_json::from_str::<RobloxBadgesLookup>(&json)?;
    Ok(badges.badges.into_iter().map(|x| x.name).collect())
}

crate fn has_player_badge(id: RobloxUserID, asset: u64) -> Result<bool> {
    let uri = format!("https://assetgame.roblox.com/Game/Badge/HasBadge.ashx?UserID={}&BadgeID={}",
                      id.0, asset);
    Ok(get_api_endpoint(&uri)?.error_for_status()?.text()? == "Success")
}

crate fn get_player_groups(id: RobloxUserID) -> Result<HashMap<u64, u32>> {
    let uri = format!("https://api.roblox.com/users/{}/groups", id.0);
    let json = get_api_endpoint(&uri)?.error_for_status()?.text()?;
    let groups = serde_json::from_str::<Vec<RobloxGroupLookup>>(&json)?;
    let mut map = HashMap::new();
    for RobloxGroupLookup { id, rank } in groups {
        map.insert(id, rank);
    }
    Ok(map)
}
