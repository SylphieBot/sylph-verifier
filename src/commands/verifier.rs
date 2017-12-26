use super::*;

use chrono::{DateTime, Utc};
use roblox::*;
use std::time::SystemTime;
use util;

fn get_discord_username(discord_id: UserId) -> String {
    match discord_id.find() {
        Some(x) => x.read().tag(),
        None => match discord_id.get() {
            Ok(x) => x.tag(),
            Err(_) => format!("(discord uid #{})", discord_id.0),
        }
    }
}
fn reverify_help(
    ctx: &CommandContext, discord_id: UserId, roblox_id: RobloxUserID
) -> Result<String> {
    if ctx.core.verifier().get_verified_roblox_user(discord_id)? == Some(roblox_id) {
        Ok(format!(" If you only want to update your roles, use the '{}update' command.",
                   ctx.prefix()))
    } else {
        Ok(String::new())
    }
}
fn do_verify(ctx: &CommandContext, discord_ctx: &Context, msg: &Message) -> Result<()> {
    let roblox_username = ctx.arg(0)?;
    let token = ctx.arg(1)?;

    let roblox_id = RobloxUserID::for_username(roblox_username)?;
    let discord_username = msg.author.tag();
    let discord_id = msg.author.id;

    debug!("Beginning verification attempt: {} -> {}", discord_username, roblox_username);

    match ctx.core.verifier().try_verify(discord_id, roblox_id, token)? {
        VerifyResult::VerificationOk => {
            info!("{} successfully verified as {}",
                  discord_username, roblox_username);
            unimplemented!()
        }
        VerifyResult::TokenAlreadyUsed => {
            info!("{} failed to verify as {}: Token already used.",
                  discord_username, roblox_username);
            cmd_error!("Someone has already used that verification code. Please wait for a \
                        new code to be generated, then try again.")
        }
        VerifyResult::VerificationPlaceOutdated => {
            info!("{} failed to verify as {}: Outdated verification place.",
                  discord_username, roblox_username);
            cmd_error!("The verification place is outdated, and has not been updated with the \
                        verification bot. Please contact the bot owner.")
        }
        VerifyResult::InvalidToken => {
            info!("{} failed to verify as {}: Invalid token.",
                  discord_username, roblox_username);
            cmd_error!("The verification code you used is not valid. Please check the code \
                        you entered and try again.")
        }
        VerifyResult::TooManyAttempts { max_attempts, cooldown, cooldown_ends } => {
            info!("{} failed to verify as {}: Too many attempts.",
                  discord_username, roblox_username);
            cmd_error!("You can only try to verify {} times every {}. \
                        Please try again in {}.{}",
                       max_attempts, util::to_english_time(cooldown),
                       util::english_time_diff(SystemTime::now(), cooldown_ends),
                       reverify_help(ctx, discord_id, roblox_id)?)
        }
        VerifyResult::SenderVerifiedAs { other_roblox_id } => {
            let other_roblox_username = other_roblox_id.lookup_username()?;
            info!("{} failed to verify as {}: Already verified as {}.",
                  discord_username, roblox_username, other_roblox_username);
            cmd_error!("You are already verified as {}.{}",
                       other_roblox_username, reverify_help(ctx, discord_id, roblox_id)?)
        }
        VerifyResult::RobloxAccountVerifiedTo { other_discord_id } => {
            let other_discord_username = get_discord_username(other_discord_id);
            info!("{} failed to verify as {}: Roblox account already verified to {}.",
                  discord_username, roblox_username, other_discord_username);
            cmd_error!("{} has already verified as {}.",
                       other_discord_username, roblox_username)
        }
        VerifyResult::ReverifyOnCooldown { cooldown, cooldown_ends } => {
            info!("{} failed to verify as {}: Reverified too soon.",
                  discord_username, roblox_username);
            cmd_error!("You can only reverify once every {}. Please try again in {}.{}",
                       util::to_english_time(cooldown),
                       util::english_time_diff(SystemTime::now(), cooldown_ends),
                       reverify_help(ctx, discord_id, roblox_id)?)
        }
    }
}

fn check_configuration(ctx: &CommandContext, guild_id: GuildId) -> Result<()> {
    if let Some(err) = ctx.core.roles().check_error(guild_id)? {
        ctx.respond(format!("The role configuration has been successfully updated. However, \
                             errors were found in the configuration: {}", err))
    } else {
        ctx.respond("The role configuration has been successfully updated.")
    }
}

// TODO: Support roles by <@id> and id.
pub const COMMANDS: &[Command] = &[
    Command::new("show_config")
        .help(None, "Shows the role configuration for the current channel.")
        .required_permissions(enum_set!(DiscordPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;

            let mut config = String::new();
            let config_map = ctx.core.roles().get_configuration(guild_id)?;
            let mut role_names: Vec<&str> = config_map.keys().map(|x| x.as_str()).collect();
            role_names.sort();
            for role in role_names {
                let role_data = &config_map[role];
                let definition = role_data.custom_rule.as_ref()
                    .map(|x| format!("`{}`", x))
                    .unwrap_or_else(|| if VerificationRule::has_builtin(role) {
                        "*(builtin)*".to_string()
                    } else {
                        "**(does not exist)**".to_string()
                    });
                writeln!(config, "• {} = {}", role, definition)?;
                if let Some((_, ref role_name)) = role_data.role_data {
                    writeln!(config, "   Users matching this rule will be assigned **{}**.",
                             role_name)?;
                }
                let date: DateTime<Utc> = role_data.last_updated.into();
                writeln!(config, "   *Last updated at {} UTC*", date.format("%Y-%m-%d %H:%M:%S"))?;
            }
            ctx.respond(config)
        }),
    Command::new("set_role")
        .help(Some("<rule name> [discord role name]"),
              "Sets the Discord role the bot will set when a rule is matched.")
        .required_permissions(enum_set!(DiscordPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let rule_name = ctx.arg(0)?;
            let role_name = ctx.rest(1)?.trim();
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;
            if !role_name.is_empty() {
                let mut found_role = None;
                {
                    let guild = guild_id.find().chain_err(|| "Guild not found.")?;
                    let guild = guild.read();
                    for (_, ref role) in &guild.roles {
                        // TODO: Check that the sender can set the role. (rank check)
                        if role.name.trim() == role_name {
                            cmd_ensure!(found_role.is_none(),
                            "Two roles named '{}' found!", role_name);
                            found_role = Some(role.id);
                        }
                    }
                }
                if let Some(role_id) = found_role {
                    ctx.core.roles().set_active_role(guild_id, rule_name, Some(role_id))?;
                } else {
                    cmd_error!("No role named '{}' found. Note that roles are case sensitive.",
                               role_name)
                }
            } else {
                ctx.core.roles().set_active_role(guild_id, rule_name, None)?;
            }
            check_configuration(ctx, guild_id)
        }),
    Command::new("set_custom_rule")
        .help(Some("<rule name> [rule definition]"),
              "Defines a custom rule for setting roles.")
        .required_permissions(enum_set!(DiscordPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let rule_name = ctx.arg(0)?;
            let definition = ctx.rest(1)?.trim();
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;
            if !definition.is_empty() {
                ctx.core.roles().set_custom_rule(guild_id, rule_name, Some(definition))?;
            } else {
                ctx.core.roles().set_custom_rule(guild_id, rule_name, None)?;
            }
            check_configuration(ctx, guild_id)
        }),
    Command::new("test_verify")
        .help(Some("<roblox username>"), "Tests the results of your role configuration.")
        .required_permissions(enum_set!(DiscordPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let roblox_username = ctx.arg(0)?;
            let roblox_id = RobloxUserID::for_username(roblox_username)?;
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;

            let mut roles = String::new();
            for role in ctx.core.roles().get_assigned_roles(guild_id, roblox_id)? {
                writeln!(roles, "• {} {} **{}**",
                         roblox_username,
                         if role.is_assigned { "will be assigned" }
                             else { "will not be assigned" },
                         role.role_name)?
            }
            if roles.is_empty() {
                ctx.respond("No roles are configured.")
            } else {
                ctx.respond(roles.trim())
            }
        }),
    Command::new("verify")
        .help(Some("<roblox username> <verification code>"),
              "Verifies a Roblox account to your Discord account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(do_verify),
];