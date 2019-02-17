use super::*;
use super::util::*;

use chrono::{DateTime, Utc};
use roblox::*;
use serenity;
use std::borrow::Cow;
use std::time::SystemTime;
use util;

// TODO: Check role existence.
// TODO: Take care of massive code redundancy here.

fn get_discord_username(discord_id: UserId) -> String {
    match discord_id.to_user_cached() {
        Some(x) => x.read().tag(),
        None => match discord_id.to_user() {
            Ok(x) => x.tag(),
            Err(_) => format!("(discord uid #{})", discord_id.0),
        }
    }
}
fn verify_status_str(prefix: &str, result: SetRolesStatus) -> Cow<'static, str> {
    match result {
        SetRolesStatus::Success {
            nickname_admin_error, determine_roles_error, set_roles_error, ..
        } => {
            if determine_roles_error || set_roles_error {
                format!(
                    "An error occurred while {}, and some of your roles may not have been set. \
                     Please wait a while then use the '{}update' command.",
                    if determine_roles_error {
                        "looking up your Roblox account"
                    } else {
                        "setting your Discord roles"
                    },
                    prefix,
                ).into()
            } else {
                format!(
                    "Your roles have been updated.{}",
                    if nickname_admin_error {
                        " Your nickname was not set as this bot does not have the permissions \
                          needed to edit it."
                    } else { "" }
                ).into()
            }
        }
        SetRolesStatus::NotVerified =>
            "Your roles were not updated as you are not verified.".into(),
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
    cmd_ensure!(ctx.argc() >= 2, ctx.core.verify_channel().verify_instructions()?);

    let roblox_username = ctx.arg(0)?;
    let token = ctx.arg(1)?;

    let roblox_id = RobloxUserID::for_username(roblox_username)?;
    let discord_username = msg.author.tag();
    let discord_id = msg.author.id;

    let discord_display = format!("{} (`{}`)", discord_username, discord_id.0);
    let roblox_display = format!("`{}` (https://www.roblox.com/users/{}/profile)",
                                 roblox_username, roblox_id.0);

    let guild_id = msg.guild_id?;

    debug!("Beginning verification attempt: {} -> {}", discord_display, roblox_display);

    let log_channel = ctx.core.config().get(None, ConfigKeys::GlobalVerificationLogChannel)?;
    macro_rules! verify_status {
        ($prefix_log:expr, $($format:tt)*) => {
            let buffer = format!($($format)*);
            info!("{}", buffer);
            if let Some(log_channel_id) = log_channel {
                let log_channel_id = ChannelId(log_channel_id);
                log_channel_id.send_message(|m|
                    m.content(format_args!(concat!("`[{}]` ", $prefix_log, "{}"),
                                           Utc::now().format("%H:%M:%S"), buffer))
                )?;
            }
        }
    }
    match ctx.core.verifier().try_verify(discord_id, roblox_id, token)? {
        VerifyResult::VerificationOk => {
            verify_status!("ℹ ", "{} successfully verified as {}",
                           discord_display, roblox_display);
        }
        VerifyResult::ReverifyOk { discord_link, roblox_link } => {
            let discord_link_display = if let Some(discord_id) = discord_link {
                let discord_username = discord_id.to_user()?.tag();
                format!("Old Discord account: {} (`{}`)", discord_username, discord_id.0)
            } else {
                format!("No old Discord account")
            };
            let roblox_link_display = if let Some(roblox_id) = roblox_link {
                let roblox_username = roblox_id.lookup_username()?;
                format!("Old Roblox account: `{}` (https://www.roblox.com/users/{}/profile)",
                        roblox_username, roblox_id.0)
            } else {
                format!("No old Roblox account")
            };
            verify_status!("⚠ ", "{} successfully reverified as {}\n{}; {}",
                           discord_display, roblox_display,
                           discord_link_display, roblox_link_display);
        }
        VerifyResult::TokenAlreadyUsed => {
            verify_status!("🛑 ", "{} failed to verify as {}: Token already used.",
                           discord_display, roblox_display);
            cmd_error!("Someone has already used that verification code. Please wait for a \
                        new code to be generated, then try again.")
        }
        VerifyResult::VerificationPlaceOutdated => {
            verify_status!("⚠⚠⚠⚠⚠ ", "{} failed to verify as {}: Outdated verification place.",
                           discord_display, roblox_display);
            cmd_error!("The verification place is outdated, and has not been updated with the \
                        verification bot. Please contact the bot owner.")
        }
        VerifyResult::InvalidToken => {
            verify_status!("🛑 ", "{} failed to verify as {}: Invalid token.",
                           discord_display, roblox_display);
            cmd_error!("The verification code you used is not valid. Please check the code \
                        you entered and try again.")
        }
        VerifyResult::TooManyAttempts { max_attempts, cooldown, cooldown_ends } => {
            verify_status!("🛑 ", "{} failed to verify as {}: Too many attempts.",
                           discord_display, roblox_display);
            cmd_error!("You can only try to verify {} times every {}. \
                        Please try again in {}.{}",
                       max_attempts, util::to_english_time(cooldown),
                       util::english_time_diff(SystemTime::now(), cooldown_ends),
                       reverify_help(ctx, discord_id, roblox_id)?)
        }
        VerifyResult::SenderVerifiedAs { other_roblox_id } => {
            let other_roblox_username = other_roblox_id.lookup_username()?;
            verify_status!("🛑 ", "{} failed to verify as {}: Already verified as {}.",
                           discord_display, roblox_display, other_roblox_username);
            cmd_error!("You are already verified as {}.{}",
                       other_roblox_username, reverify_help(ctx, discord_id, roblox_id)?)
        }
        VerifyResult::RobloxAccountVerifiedTo { other_discord_id } => {
            let other_discord_username = get_discord_username(other_discord_id);
            verify_status!("🛑 ", "{} failed to verify as {}: Roblox account already verified to {}.",
                           discord_display, roblox_display, other_discord_username);
            cmd_error!("{} has already verified as {}.",
                       other_discord_username, roblox_username)
        }
        VerifyResult::ReverifyOnCooldown { cooldown, cooldown_ends } => {
            verify_status!("🛑 ", "{} failed to verify as {}: Reverified too soon.",
                           discord_display, roblox_display);
            cmd_error!("You can only reverify once every {}. Please try again in {}.{}",
                       util::to_english_time(cooldown),
                       util::english_time_diff(SystemTime::now(), cooldown_ends),
                       reverify_help(ctx, discord_id, roblox_id)?)
        }
    }

    let status = ctx.core.roles().assign_roles(guild_id, msg.author.id, Some(roblox_id))?;
    ctx.respond(verify_status_str(ctx.prefix(), status))?;
    Ok(())
}

fn check_configuration(ctx: &CommandContext, guild_id: GuildId) -> Result<()> {
    if let Some(err) = ctx.core.roles().check_error(guild_id)? {
        ctx.respond(format!("The role configuration has been successfully updated. However, \
                             errors were found in the configuration: {}", err))
    } else {
        ctx.respond("The role configuration has been successfully updated.")
    }
}

fn whois_msg(
    ctx: &CommandContext, user: User, roblox_id: RobloxUserID, roblox_name: &str
) -> Result<()> {
    ctx.respond(format!("{} (`{}`) is verified as {} (https://www.roblox.com/users/{}/profile)",
                        user.tag(), user.id.0, roblox_name, roblox_id.0))
}
fn whois_discord(ctx: &CommandContext, discord_user_id: UserId) -> Result<()> {
    let user = discord_user_id.to_user().map_err(Error::from)
        .status_to_cmd(StatusCode::NotFound, || "That Discord account does not exist.")?;
    let roblox_user_id = ctx.core.verifier().get_verified_roblox_user(discord_user_id)?;
    if let Some(roblox_user_id) = roblox_user_id {
        let roblox_name = roblox_user_id.lookup_username_opt()?;
        if let Some(roblox_name) = roblox_name {
            whois_msg(ctx, user, roblox_user_id, &roblox_name)
        } else {
            cmd_error!("{} (`{}`) is verified as Roblox User ID #{}, which no longer exists. \
                        (https://www.roblox.com/users/{}/profile)",
                       user.tag(), user.id.0, roblox_user_id.0, roblox_user_id.0)
        }
    } else {
        cmd_error!("{} (`{}`) isn't verified.", user.tag(), user.id.0)
    }
}
fn whois_roblox(ctx: &CommandContext, roblox_name: &str) -> Result<()> {
    let roblox_user_id = RobloxUserID::for_username(roblox_name)?;
    let discord_user_id = ctx.core.verifier().get_verified_discord_user(roblox_user_id)?;
    if let Some(discord_user_id) = discord_user_id {
        let user = discord_user_id.to_user().map_err(Error::from)
            .status_to_cmd(StatusCode::NotFound, ||
                format!("The Discord account verified with '{}' no longer exists.", roblox_name)
            )?;
        whois_msg(ctx, user, roblox_user_id, roblox_name)
    } else {
        cmd_error!("No Discord user has verified as {} (https://www.roblox.com/users/{}/profile)",
                   roblox_name, roblox_user_id.0)
    }
}
fn do_whois(ctx: &CommandContext) -> Result<()> {
    let target_name = ctx.arg(0)?;
    if let Some(user_id) = find_user(target_name)? {
        whois_discord(ctx, user_id)
    } else {
        whois_roblox(ctx, target_name)
    }
}

fn format_discord_id(id: UserId) -> String {
    match id.to_user() {
        Ok(user) => user.tag(),
        Err(_) => format!("*(non-existent Discord id #{})*", id.0),
    }
}
fn format_roblox_id(id: RobloxUserID) -> String {
    match id.lookup_username() {
        Ok(username) => username,
        Err(_) => format!("*(non-existent Roblox id #{})*", id.0),
    }
}
fn display_history<T>(
     ctx: &CommandContext, entries: Vec<HistoryEntry<T>>,
     header_name: &str, to_string: impl Fn(T) -> String,
) -> Result<()> {
    let mut history = String::new();
    writeln!(history, "History for {}:", header_name)?;
    for entry in entries {
        let date: DateTime<Utc> = entry.last_updated.into();
        writeln!(history, "• Account was {} with {} on {} UTC",
                 if entry.is_unverify { "unverified" } else { "verified" },
                 to_string(entry.id), date.format("%Y-%m-%d %H:%M:%S"))?;
    }
    ctx.respond(history)
}
fn do_whowas(ctx: &CommandContext) -> Result<()> {
    let target_name = ctx.arg(0)?;
    if let Some(user_id) = find_user(target_name)? {
        display_history(ctx, ctx.core.verifier().get_discord_user_history(user_id, 10)?,
                        &format!("Discord user {}", format_discord_id(user_id)), format_roblox_id)
    } else {
        let roblox_id = RobloxUserID::for_username(target_name)?;
        display_history(ctx, ctx.core.verifier().get_roblox_user_history(roblox_id, 10)?,
                        &format!("Roblox user {}", format_roblox_id(roblox_id)), format_discord_id)
    }
}

fn force_unverify(
    ctx: &CommandContext, discord_id: UserId, roblox_id: RobloxUserID,
) -> Result<()> {
    let user = discord_id.to_user().map_err(Error::from)
        .status_to_cmd(StatusCode::NotFound, || "That Discord account does not exist.")?;
    ctx.core.verifier().unverify(discord_id)?;
    ctx.respond(format!("User {} has been unverified with {}.",
                        user.tag(), roblox_id.lookup_username()?))
}
fn do_force_unverify(ctx: &CommandContext) -> Result<()> {
    let target_name = ctx.arg(0)?;
    if let Some(discord_id) = find_user(target_name)? {
        let roblox_id = ctx.core.verifier().get_verified_roblox_user(discord_id)?;
        if let Some(roblox_id) = roblox_id {
            force_unverify(ctx, discord_id, roblox_id)
        } else {
            cmd_error!("User isn't verified.");
        }
    } else {
        let roblox_id = RobloxUserID::for_username(target_name)?;
        let discord_id = ctx.core.verifier().get_verified_discord_user(roblox_id)?;
        if let Some(discord_id) = discord_id {
            force_unverify(ctx, discord_id, roblox_id)
        } else {
            cmd_error!("No Discord user has verified as {}.", target_name);
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

crate const COMMANDS: &[Command] = &[
    Command::new("show_config")
        .help(None, "Shows the role configuration for the current channel.")
        .required_permissions(enum_set!(BotPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id?;

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
                    let guild = guild_id.to_guild_cached()?;
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
        .required_permissions(enum_set!(BotPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let rule_name = ctx.arg(0)?;
            let role_name = ctx.rest(1)?.trim();
            let guild_id = msg.guild_id?;
            let my_id = serenity::CACHE.read().user.id;
            if !role_name.is_empty() {
                let role_id = find_role(guild_id, role_name)?;
                if !ctx.has_permissions(BotPermission::BypassHierarchy.into()) &&
                   !util::can_member_access_role(guild_id, msg.author.id, role_id)? {
                    cmd_error!("You do not have permission to modify that role.")
                }
                if !util::can_member_access_role(guild_id, my_id, role_id)? {
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
        .required_permissions(enum_set!(BotPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let rule_name = ctx.arg(0)?;
            let definition = ctx.rest(1)?.trim();
            let guild_id = msg.guild_id?;
            if !definition.is_empty() {
                ctx.core.roles().set_custom_rule(guild_id, rule_name, Some(definition))?;
            } else {
                ctx.core.roles().set_custom_rule(guild_id, rule_name, None)?;
            }
            check_configuration(ctx, guild_id)
        }),
    Command::new("test_verify")
        .help(Some("<roblox username>"), "Tests the results of your role configuration.")
        .required_permissions(enum_set!(BotPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let roblox_username = ctx.arg(0)?;
            let roblox_id = RobloxUserID::for_username(roblox_username)?;
            let guild_id = msg.guild_id?;

            let mut roles = String::new();
            for role in ctx.core.roles().get_assigned_roles(guild_id, roblox_id)? {
                writeln!(roles, "• {}{} {} **{}**",
                         if role.is_assigned == RuleResult::Error {
                             "An error occurred while determining if "
                         } else {
                             ""
                         },
                         roblox_username,
                         match role.is_assigned {
                             RuleResult::True | RuleResult::Error => "matches the rule",
                             RuleResult::False => "does not match the rule",
                         },
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
            cmd_ensure!(ctx.core.verifier().get_verified_roblox_user(msg.author.id)?.is_some(),
                        "You are not verified with this bot. {}",
                        ctx.core.verify_channel().verify_instructions()?);

            let guild_id = msg.guild_id?;
            let cooldown =
                ctx.core.config().get(Some(guild_id), ConfigKeys::UpdateCooldownSeconds)?;
            let status = ctx.core.roles().update_user_with_cooldown(
                guild_id, msg.author.id, cooldown, true, false,
            )?;
            ctx.respond(verify_status_str(ctx.prefix(), status))?;
            Ok(())
        }),
    Command::new("whois")
        .help(Some("<discord mention, user id, or roblox username>"),
              "Retrieves the Roblox account a Discord account is verified with or vice versa.")
        .required_permissions(enum_set!(BotPermission::Whois))
        .exec(do_whois),
    Command::new("whowas")
        .help(Some("<discord mention, user id, or roblox username>"),
              "Retrieves the verification history of a Roblox account or a Discord account.")
        .required_permissions(enum_set!(BotPermission::Whowas))
        .exec(do_whowas),
    Command::new("verify")
        .help(Some("<roblox username> <verification code>"),
              "Verifies a Roblox account to your Discord account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(do_verify),
    Command::new("unverify")
        .help(None, "Unverifies your Roblox account from your Discord account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .required_permissions(enum_set!(BotPermission::Unverify))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id?;
            ctx.core.verifier().unverify(msg.author.id)?;
            let status = ctx.core.roles().assign_roles(guild_id, msg.author.id, None)?;
            ctx.respond(verify_status_str(ctx.prefix(), status))
        }),
    Command::new("force_unverify")
        .help(None, "Unverifies your Roblox account from your Discord account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .required_permissions(enum_set!(BotPermission::UnverifyOther))
        .exec(do_force_unverify),
    Command::new("set_verification_channel")
        .help(None, "Makes the current channel a verification channel.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .required_permissions(enum_set!(BotPermission::ManageGuildSettings))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id?;
            if let Some("confirm") = ctx.arg_opt(0) {
                ctx.core.verify_channel().setup(guild_id, msg.channel_id)?;
            } else {
                ctx.core.verify_channel().setup_check(guild_id, msg.channel_id)?;
                ctx.respond(format!(
                    "You are setting this channel to be a verification channel. This will cause \
                     the bot to:\n\
                     • Delete all messages currently in in this channel.\n\
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
        .required_permissions(enum_set!(BotPermission::ManageGuildSettings))
        .exec_discord(|ctx, _, msg| {
            let guild_id = msg.guild_id?;
            ctx.core.verify_channel().remove(guild_id)?;
            Ok(())
        }),
    Command::new("explain")
        .help(Some("[rule to explain]"),
              "Explains the compilation of your ruleset or a role. You probably don't need this.")
        .required_permissions(enum_set!(BotPermission::ManageRoles))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| {
            let rule = ctx.rest(0)?;
            if rule == "" {
                let guild_id = msg.guild_id?;
                maybe_sprunge(ctx, &ctx.core.roles().explain_rule_set(guild_id)?)
            } else {
                let rule = format!("{}", VerificationRule::from_str(rule)?);
                maybe_sprunge(ctx, &rule)
            }
        }),
];