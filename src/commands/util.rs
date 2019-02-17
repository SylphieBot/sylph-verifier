use super::*;

use regex::Regex;

lazy_static! {
    static ref MENTION_REGEX: Regex = Regex::new("^<@!?([0-9]+)>$").unwrap();
    static ref SNOWFLAKE_REGEX: Regex = Regex::new("^([0-9]+)$").unwrap();
}

crate fn find_role(guild_id: GuildId, role_name: &str) -> Result<RoleId> {
    let guild = guild_id.to_guild_cached()?;
    let guild = guild.read();

    if let Some(captures) = MENTION_REGEX.captures(role_name) {
        let role_id_str = captures.get(1)?.as_str();
        let role_id = RoleId(role_id_str.parse().to_cmd_err(|| "Role ID too large.")?);
        cmd_ensure!(guild.roles.contains_key(&role_id),
                    "That role does not exist in this server.");
        Ok(role_id)
    } else {
        let mut found_role = None;
        for role in guild.roles.values() {
            if role.name.trim() == role_name {
                cmd_ensure!(found_role.is_none(),
                            "Two roles named '{}' found! Consider using `<@role id>` \
                             to disambiguate.", role_name);
                found_role = Some(role.id);
            }
        }
        match found_role {
            Some(role) => Ok(role),
            None => cmd_error!("No role named '{}' found. Note that roles are case sensitive.",
                               role_name),
        }
    }
}

crate fn find_user(user_name: &str) -> Result<Option<UserId>> {
    if let Some(captures) = MENTION_REGEX.captures(user_name) {
        let user_id_str = captures.get(1)?.as_str();
        Ok(Some(UserId(user_id_str.parse().to_cmd_err(|| "User ID too large.")?)))
    } else if SNOWFLAKE_REGEX.is_match(user_name) {
        Ok(Some(UserId(user_name.parse().to_cmd_err(|| "User ID too large.")?)))
    } else {
        Ok(None)
    }
}