use super::*;

use core::VerifierCore;
use std::fmt::Display;
use util;

#[derive(Copy, Clone, Eq, PartialEq)]
enum GuildShowType {
    AlwaysShow, OnlyInTerminal, OnlyInGuild,
}
impl GuildShowType {
    fn show_in(self, guild: Option<GuildId>) -> bool {
        match self {
            GuildShowType::AlwaysShow     => true,
            GuildShowType::OnlyInTerminal => guild.is_none(),
            GuildShowType::OnlyInGuild    => guild.is_some(),
        }
    }
}

fn parse_as<R : FromStr>(s: &str, err: &str) -> Result<R> {
    match s.parse() {
        Ok(r) => Ok(r),
        Err(_) => cmd_error!("{}", err),
    }
}
fn parse_bool(s: &str) -> Result<bool> {
    parse_as(s, "Setting must be true or false.")
}
fn parse_u32(s: &str) -> Result<u32> {
    parse_as(s, "Setting must be a non-negative number.")
}
fn parse_u64(s: &str) -> Result<u64> {
    parse_as(s, "Setting must be a non-negative number.")
}
fn print_display(_: &VerifierCore, t: impl Display) -> Result<String> {
    Ok(format!("{}", t))
}
fn print_quoted(_: &VerifierCore, t: impl Display) -> Result<String> {
    Ok(format!("\"{}\"", t))
}

macro_rules! config_values {
    ($($config_name:ident<$tp:ty>(
        $config_key:ident, $allow_guild:expr, $show_type:expr,
        $help:expr, $from_str:expr, $to_str:expr $(,)*
    );)*) => {
        fn set_config(
            core: &VerifierCore, guild: Option<GuildId>, key: &str, value: Option<&str>
        ) -> Result<()> {
            match key {
                $(
                    stringify!($config_name) => {
                        cmd_ensure!(guild.is_none() || $allow_guild,
                                    "This option cannot be set per-server.");
                        match value {
                            Some(str) => {
                                let from_str: fn(&str) -> Result<$tp> = $from_str;
                                core.config().set(core, guild,
                                                  ConfigKeys::$config_key, from_str(str)?)?;
                            }
                            None => core.config().reset(core, guild, ConfigKeys::$config_key)?,
                        }
                    }
                )*
                name => cmd_error!("No such configuration option '{}'.", name),
            }
            Ok(())
        }

        fn print_config(core: &VerifierCore, guild: Option<GuildId>) -> Result<String> {
            let mut config = String::new();
            let align = if guild.is_some() { "   " } else { "  " };
            $({
                if $show_type.show_in(guild) {
                    let to_str: fn(&VerifierCore, $tp) -> Result<String> = $to_str;
                    let value = core.config().get(guild, ConfigKeys::$config_key)?;
                    writeln!(config, "• {} = {}{}",
                             stringify!($config_name), to_str(core, value)?,
                             if guild.is_some() && !$allow_guild {
                                " *(This option cannot be overwritten per-server.)*".to_owned()
                             } else {
                                String::new()
                             })?;
                    writeln!(config, "{}{}", align, $help)?;
                }
            })*
            Ok(config)
        }
    }
}
config_values! {
    prefix<String>(
        CommandPrefix, false, GuildShowType::AlwaysShow,
        "The prefix used before commands.",
        |x| Ok(x.to_owned()), print_quoted);
    discord_token<Option<String>>(
        DiscordToken, false, GuildShowType::OnlyInTerminal,
        "The bot token used to connect to Discord.",
        |x|    Ok(Some(x.to_owned())),
        |_, x| Ok(x.map_or("(not set)", |_| "<token redacted>").to_owned()));

    set_nickname<bool>(
        SetNickname, true, GuildShowType::OnlyInGuild,
        "Whether to set a user's nickname to their Roblox username while updating their roles.",
        parse_bool, print_display);
    set_roles_on_join<bool>(
        SetRolesOnJoin, true, GuildShowType::OnlyInGuild,
        "Whether to set a user's roles on server join based on an existing verification.",
        parse_bool, print_display);
    auto_update_roles<bool>(
        EnableAutoUpdate, true, GuildShowType::OnlyInGuild,
        "Whether to periodically update a user's roles when they talk.",
        parse_bool, print_display);
    auto_update_unverified_roles<bool>(
        EnableAutoUpdateUnverified, true, GuildShowType::OnlyInGuild,
        "Whether the bot will auto-update the roles of users who aren't verified \
         (i.e. remove them).",
        parse_bool, print_display);

    update_cooldown<u64>(
        UpdateCooldownSeconds, true, GuildShowType::OnlyInGuild,
        "The number of seconds a user must wait between manual role updates.",
        parse_u64, |_, x| Ok(util::to_english_time_precise(x)));
    auto_update_cooldown<u64>(
        AutoUpdateCooldownSeconds, true, GuildShowType::OnlyInGuild,
        "The number of seconds between automatic role updates.",
        parse_u64, |_, x| Ok(util::to_english_time_precise(x)));

    place_ui_title<String>(
        PlaceUITitle, false, GuildShowType::OnlyInTerminal,
        "The title of the verification place UI.",
        |x| Ok(x.to_owned()), print_quoted);
    place_ui_instructions<String>(
        PlaceUIInstructions, false, GuildShowType::OnlyInTerminal,
        "The instructions shown in the verification place UI.",
        |x| Ok(x.to_owned()), print_quoted);
    place_ui_background<Option<String>>(
        PlaceUIBackground, false, GuildShowType::OnlyInTerminal,
        "The Asset ID shown in the verification place UI background.",
        |x|    Ok(Some(x.to_owned())),
        |_, x| Ok(x.unwrap_or_else(|| "(default)".to_owned())));
    place_id<Option<u64>>(
        PlaceID, false, GuildShowType::OnlyInTerminal,
        "The ID of the verification place. This is displayed in verification channel messages.",
        |x| parse_u64(x).map(Some),
        |_, x| Ok(x.map_or_else(|| "*(none set)*".to_owned(), |x| format!("{}", x))));

    verification_attempt_limit<u32>(
        VerificationAttemptLimit, false, GuildShowType::OnlyInTerminal,
        "How many times a user can verify in a row before they must wait a period of time.",
        parse_u32, print_display);
    verification_cooldown<u64>(
        VerificationCooldownSeconds, false, GuildShowType::OnlyInTerminal,
        "How many seconds a user must wait to attempt to verify after using up the attempt limit.",
        parse_u64, |_, x| Ok(util::to_english_time_precise(x)));

    verification_channel_intro<Option<String>>(
        VerificationChannelIntro, true, GuildShowType::OnlyInGuild,
        "A sentence that you can add to the start of your server's verification message.",
        |x| Ok(Some(x.to_owned())),
        |_, x| Ok(x.map_or_else(|| "*(none set)*".to_owned(), |x| format!("\"{}\"", x))));
    verification_channel_delete_timer<u32>(
        VerificationChannelDeleteSeconds, true, GuildShowType::OnlyInGuild,
        "How many seconds to wait before deleting responses in a verification channel.",
        |x| {
            let secs = parse_u32(x)?;
            cmd_ensure!(secs < 60 * 5, "Maximum delete wait is 5 minutes.");
            Ok(secs)
        },
        |_, x| Ok(util::to_english_time_precise(x as u64)));
    verification_channel_footer<Option<String>>(
        VerificationChannelFooter, true, GuildShowType::OnlyInGuild,
        "A sentence that you can add to the bottom of your server's verification message.",
        |x| Ok(Some(x.to_owned())),
        |_, x| Ok(x.map_or_else(|| "*(none set)*".to_owned(), |x| format!("\"{}\"", x))));

    token_validity<u32>(
        TokenValiditySeconds, false, GuildShowType::OnlyInTerminal,
        "How many seconds a verification token is valid for.",
        parse_u32, |_, x| Ok(util::to_english_time_precise(x as u64)));

    allow_reverify_discord_account<bool>(
        AllowReverifyDiscord, false, GuildShowType::OnlyInTerminal,
        "Whether a user can reverify a Discord account that is already verified.",
        parse_bool, print_display);
    allow_reverify_roblox_account<bool>(
        AllowReverifyRoblox, false, GuildShowType::OnlyInTerminal,
        "Whether a user can reverify a Roblox account that is already verified.",
        parse_bool, print_display);
    reverification_cooldown<u64>(
        ReverificationCooldownSeconds, false, GuildShowType::OnlyInTerminal,
        "How many seconds a user must wait after verifying before they can reverify.",
        parse_u64, |_, x| Ok(util::to_english_time_precise(x)));
}

fn set(ctx: &CommandContext, guild: Option<GuildId>) -> Result<()> {
    if ctx.argc() == 0 {
        ctx.respond(print_config(ctx.core, guild)?)
    } else {
        let key = ctx.arg(0)?;
        let value = ctx.rest(1)?;
        if value.trim().is_empty() {
            set_config(ctx.core, guild, key, None)?;
            ctx.respond("Configuration option reset to default.")?;
        } else {
            set_config(ctx.core, guild, key, Some(value))?;
            ctx.respond("Configuration option set.")?;
        }
        Ok(())
    }
}

crate const COMMANDS: &[Command] = &[
    Command::new("set")
        .help(Some("<key> [new value]"), "Sets a configuration value for this server.")
        .required_permissions(enum_set!(BotPermission::ManageGuildSettings))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec(|ctx| set(ctx, Some(ctx.get_guild()?.unwrap()))),
    Command::new("set_global")
        .help(Some("<key> [new value]"), "Sets a global configuration value.")
        .required_permissions(enum_set!(BotPermission::ManageGlobalSetings))
        .exec(|ctx| set(ctx, None)),
];