use super::*;

use chrono::{DateTime, Utc};
use regex::Regex;
use roblox::*;
use serenity;
use std::cmp::max;
use std::time::SystemTime;
use util;

// TODO: Check role existence.
// TODO: Consider moving error messages back into roles.rs
// TODO: Suggest proper usage when people are using it wrong.
// TODO: Support force updating an user.

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
fn do_verify(ctx: &CommandContext, _: &Context, msg: &Message) -> Result<()> {
    let roblox_username = ctx.arg(0)?;
    let token = ctx.arg(1)?;

    let roblox_id = RobloxUserID::for_username(roblox_username)?;
    let discord_username = msg.author.tag();
    let discord_id = msg.author.id;

    let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;

    debug!("Beginning verification attempt: {} -> {}", discord_username, roblox_username);

    match ctx.core.verifier().try_verify(discord_id, roblox_id, token)? {
        VerifyResult::VerificationOk => {
            info!("{} successfully verified as {}",
                  discord_username, roblox_username);
            match ctx.core.roles().assign_roles(guild_id, msg.author.id, Some(roblox_id))? {
                SetRolesStatus::Success =>
                    ctx.respond("Your roles have been set.")?,
                SetRolesStatus::IsAdmin =>
                    ctx.respond("Your roles have been set. Note that your nickname has not been \
                                 set, as you outrank the bot's roles, and this bot does not have \
                                 permission to edit your nickname.")?,
            }
            Ok(())
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

fn find_role(guild_id: GuildId, role_name: &str) -> Result<RoleId> {
    let guild = guild_id.find().chain_err(|| "Guild not found.")?;
    let guild = guild.read();

    lazy_static! {
        static ref MATCH_ROLE: Regex = Regex::new("^<@([0-9]+)>$").unwrap();
    }

    if let Some(captures) = MATCH_ROLE.captures(role_name) {
        let role_id_str = captures.get(1).chain_err(|| "No capture found.")?.as_str();
        let role_id = RoleId(role_id_str.parse().to_cmd_err(|| "Role ID too large.")?);
        cmd_ensure!(guild.roles.contains_key(&role_id),
                    "That role does not exist in this server.");
        Ok(role_id)
    } else {
        let mut found_role = None;
        for (_, ref role) in &guild.roles {
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

fn maybe_sprunge(ctx: &CommandContext, text: &str) -> Result<()> {
    if text.chars().count() < 1900 {
        ctx.respond(format!("```\n{}\n```", text))
    } else {
        ctx.respond(format!("The response is too long and has been uploaded to a site: {}",
                            util::sprunge(text)?))
    }
}

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
                if let Some(role_id) = role_data.role_id {
                    let guild = guild_id.find().chain_err(|| "Guild not found.")?;
                    let guild = guild.read();
                    match guild.roles.get(&role_id) {
                        Some(role) =>
                            writeln!(config, "   Users matching this rule will be assigned **{}**.",
                                     role.name)?,
                        None =>
                            writeln!(config, "   **A role with ID #{} was assigned to this rule, \
                                                 but it no longer exists!**", role_id)?,
                    };
                }
                let date: DateTime<Utc> = role_data.last_updated.into();
                writeln!(config, "   *Last updated at {} UTC*", date.format("%Y-%m-%d %H:%M:%S"))?;
            }
            if config.is_empty() {
                ctx.respond("No roles are configured.")
            } else {
                ctx.respond(config.trim())
            }
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
            let me_member = guild_id.member(serenity::CACHE.read().user.id)?;
            let sender_member = guild_id.member(msg.author.id)?;
            if !role_name.is_empty() {
                let role_id = find_role(guild_id, role_name)?;
                if ctx.privilege_level < PrivilegeLevel::BotOwner {
                    if !util::can_member_access_role(&sender_member, role_id)? {
                        cmd_error!("You do not have permission to modify that role.")
                    }
                }
                if !util::can_member_access_role(&me_member, role_id)? {
                    cmd_error!("This bot does not have permission to modify that role.")
                }
                ctx.core.roles().set_active_role(guild_id, rule_name, Some(role_id))?;
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
                         if role.is_assigned { "matches the rule" }
                             else { "does not match the rule" },
                         role.rule)?
            }
            if roles.is_empty() {
                ctx.respond("No roles are configured.")
            } else {
                ctx.respond(roles.trim())
            }
        }),
    Command::new("update")
        .help(None, "Updates your roles according to your Roblox account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;
            let cooldown = max(
                ctx.core.config().get(None, ConfigKeys::MinimumUpdateCooldownSeconds)?,
                ctx.core.config().get(Some(guild_id), ConfigKeys::UpdateCooldownSeconds)?,
            );
            ctx.core.roles().update_user_with_cooldown(guild_id, msg.author.id, cooldown, true)?;
            ctx.respond("Your roles have been updated.")?;
            Ok(())
        }),
    Command::new("verify")
        .help(Some("<roblox username> <verification code>"),
              "Verifies a Roblox account to your Discord account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(do_verify),
    Command::new("set_verification_channel")
        .help(None, "Makes the current channel a verification channel.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .required_permissions(enum_set!(DiscordPermission::ManageGuild |
                                        DiscordPermission::ManageMessages))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;
            if let Some("confirm") = ctx.arg_opt(0) {
                ctx.core.verify_channel().setup(guild_id, msg.channel_id)?;
            } else {
                ctx.core.verify_channel().setup_check(guild_id, msg.channel_id)?;
                ctx.respond(format!(
                    "You are setting this channel to be a verification channel. This will cause \
                     the bot to:\n\
                     • Delete all messages currently in in this channel.\n\
                     • Ignore commands other than those involved in verification in \
                       this channel.\n\
                     • Delete any messages sent by other users in this channel immediately \
                       after receiving them.\n\
                     • Automatically delete message sent by it in this channel after \
                       a certain period of time.\n\
                     \n\
                     Please use `{}set_verification_channel confirm` to verify that you wish to \
                     do this.", ctx.prefix(),
                ))?;
            }
            Ok(())
        }),
    Command::new("remove_verification_channel")
        .help(None, "Unsets the server's current verification channel, if one exists.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .required_permissions(enum_set!(DiscordPermission::ManageGuild |
                                        DiscordPermission::ManageMessages))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;
            ctx.core.verify_channel().remove(guild_id)?;
            Ok(())
        }),
    Command::new("explain")
        .help(Some("[rule to explain]"),
              "Explains the compilation of your ruleset or a role. You probably don't need this.")
        .required_permissions(enum_set!(DiscordPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let rule = ctx.rest(0)?;
            if rule == "" {
                let guild_id = msg.guild_id().chain_err(|| "Guild ID not found.")?;
                maybe_sprunge(ctx, &ctx.core.roles().explain_rule_set(guild_id)?)
            } else {
                let rule = format!("{}", VerificationRule::from_str(rule)?);
                maybe_sprunge(ctx, &rule)
            }
        }),
];