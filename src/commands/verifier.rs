use super::*;

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

// TODO: Support roles by <@id> and id.
// TODO: Check validity after configuration commands.
pub const COMMANDS: &[Command] = &[
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
                    for (role_id, ref role) in &guild.roles {
                        // TODO: Check that the sender can set the role. (rank check)
                        if role.name.trim() == role_name {
                            cmd_ensure!(found_role.is_none(),
                            "Two roles named '{}' found!", role_name);
                            found_role = Some(role.id);
                        }
                    }
                }
                if let Some(role_id) = found_role {
                    ctx.core.roles().set_active_role(guild_id, rule_name, Some(role_id))
                } else {
                    cmd_error!("No role named '{}' found. Note that roles are case sensitive.",
                               role_name)
                }
            } else {
                ctx.core.roles().set_active_role(guild_id, rule_name, None)
            }
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
                ctx.core.roles().set_custom_rule(guild_id, rule_name, Some(definition))
            } else {
                ctx.core.roles().set_custom_rule(guild_id, rule_name, None)
            }
        }),
    Command::new("verify")
        .help(Some("<roblox username> <verification code>"),
              "Verifies a Roblox account to your Discord account.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(do_verify),
];