use super::*;

use std::process::exit;

const LOG_LEVEL_FILTER_ERROR: &str =
    "Could not parse log level. [valid levels: trace, debug, info, warn, error, off]";

fn parse_as<R : FromStr>(s: &str, err: &str) -> Result<R> {
    match s.parse() {
        Ok(r) => Ok(r),
        Err(_) => cmd_error!("{}", err),
    }
}

struct ConfigOption {
    name: &'static str, help: &'static str, allow_guild: bool,
    set_config: fn(&CommandContext, Option<GuildId>, Option<&str>) -> Result<()>,
    get_config: fn(&CommandContext, Option<GuildId>) -> Result<String>,
}

macro_rules! config_values {
    ($($config_name:ident<$tp:ty>(
        $config_key:ident, $allow_guild:expr, $help:expr,
        $from_str:expr, $to_str:expr, $after_update:expr $(,)*
    );)*) => {
        static CONFIG_OPTION_LIST: &'static [ConfigOption] = &[
            $(
                ConfigOption {
                    name: stringify!($config_name), help: $help, allow_guild: $allow_guild,
                    set_config: |ctx, guild, val| {
                        let key = ConfigKeys::$config_key;
                        match val {
                            Some(str) => {
                                let from_str: fn(&str) -> Result<$tp> = $from_str;
                                ctx.core.config().set(guild, key, from_str(str)?)?
                            }
                            None => ctx.core.config().reset(guild, key)?,
                        }
                        let after_update: fn(&CommandContext) -> Result<()> = $after_update;
                        after_update(ctx)
                    },
                    get_config: |ctx, guild| {
                        let key = ConfigKeys::$config_key;
                        let val = ctx.core.config().get(guild, key)?;
                        let to_str: fn($tp) -> Result<String> = $to_str;
                        to_str(val)
                    },
                },
            )*
        ];
    }
}
config_values! {
    prefix<String>(
        CommandPrefix, false, "The prefix used before commands.",
        |x| Ok(x.to_owned()),
        |x| Ok(format!("\"{}\"", x)),
        |_| Ok(()));
    discord_token<Option<String>>(
        DiscordToken, false, "The bot token used to connect to Discord.",
        |x| Ok(Some(x.to_owned())),
        |x| Ok(x.map_or("(not set)", |_| "<token redacted>").to_owned()),
        |_| Ok(()));

    token_validity<u32>(
        TokenValiditySeconds, false, "How many seconds a verification token is valid for.",
        |x| parse_as(x, "Setting must be a positive number."),
        |x| Ok(format!("{}", x)),
        |x| {
            x.core.verifier().rekey(false)?;
            x.core.refresh_place()?;
            Ok(())
        });

    place_ui_title<String>(
        PlaceUITitle, false, "The title of the place UI.",
        |x| Ok(x.to_owned()),
        |x| Ok(format!("\"{}\"", x)),
        |x| x.core.refresh_place());
    place_ui_instructions<String>(
        PlaceUIInstructions, false, "The instructions shown in the place UI.",
        |x| Ok(x.to_owned()),
        |x| Ok(format!("\"{}\"", x)),
        |x| x.core.refresh_place());
    place_ui_background<Option<String>>(
        PlaceUIBackground, false, "The Asset ID shown in the background of the place UI.",
        |x| Ok(Some(x.to_owned())),
        |x| Ok(x.unwrap_or_else(|| "(default)".to_owned())),
        |x| x.core.refresh_place());
}
lazy_static! {
    static ref CONFIG_OPTIONS: HashMap<&'static str, &'static ConfigOption> = {
        let mut map = HashMap::new();
        for option in CONFIG_OPTION_LIST {
            map.insert(option.name, option);
        }
        map
    };
    static ref SORTED_OPTIONS: Vec<&'static ConfigOption> = {
        let mut vec = Vec::new();
        let mut keys: Vec<&'static str> = CONFIG_OPTIONS.keys().map(|x| *x).collect();
        keys.sort();
        for key in keys {
            vec.push(*CONFIG_OPTIONS.get(key).unwrap());
        }
        vec
    };
}

fn check_option_access(option: &ConfigOption, guild: Option<GuildId>) -> Result<()> {
    match guild {
        Some(guild) => {
            cmd_ensure!(option.allow_guild, "This option cannot be set per-guild.");
            Ok(())
        }
        None => Ok(()),
    }
}
fn set_config(ctx: &CommandContext, guild: Option<GuildId>) -> Result<()> {
    if ctx.argc() == 0 {
        let mut config = String::new();
        writeln!(config, "Current configuration options:")?;
        for &option in SORTED_OPTIONS.iter() {
            if guild.is_none() || option.allow_guild {
                writeln!(config, "• {} = {}", option.name, (option.get_config)(ctx, guild)?)?;
                writeln!(config, "  {}", option.help)?;
            }
        }
        ctx.respond(&config)
    } else {
        let config = ctx.arg_raw(0)?;
        match CONFIG_OPTIONS.get(config) {
            Some(option) => {
                cmd_ensure!(guild.is_none() || option.allow_guild,
                            "This option cannot be set per-guild.");

                let rest = ctx.rest(1)?;
                (option.set_config)(
                    ctx, guild, if rest.is_empty() { None } else { Some(rest) }
                )
            }
            None => cmd_error!("No such configuration option '{}'.", config),
        }
    }
}

pub const COMMANDS: &[Command] = &[
    Command::new("shutdown")
        .help(Some("[--force]"), "Shuts down the bot.")
        .no_threading()
        .terminal_only()
        .exec(|ctx| {
            match ctx.arg_opt_raw(0) {
                Some("--force") => { exit(1); }
                _ => { ctx.core.shutdown().ok(); }
            }
            Ok(())
        }),

    // Configuration
    Command::new("set")
        .help(Some("<key> [new value]"), "Sets a configuration value for this guild.")
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec(|ctx| set_config(ctx, Some(ctx.get_guild()?.unwrap()))),
    Command::new("set_global")
        .help(Some("<key> [new value]"), "Sets a global configuration value.")
        .terminal_only()
        .exec(|ctx| set_config(ctx, None)),
    Command::new("rekey")
        .help(None, "Changes the shared key used by the verifier.")
        .terminal_only()
        .exec(|ctx| {
            ctx.core.verifier().rekey(true)?;
            ctx.core.refresh_place()?;
            Ok(())
        }),

    // Discord management
    Command::new("connect")
        .help(None, "Connects to Discord.")
        .terminal_only()
        .exec(|ctx| {
            ctx.core.connect_discord()
        }),
    Command::new("disconnect")
        .help(None, "Disconnects from Discord.")
        .terminal_only()
        .exec(|ctx| {
            ctx.core.disconnect_discord()
        }),
    Command::new("reconnect")
        .help(None, "Reconnects to Discord.")
        .terminal_only()
        .exec(|ctx| {
            ctx.core.reconnect_discord()
        }),

    // Debugging command
    Command::new("debug_cmd")
        .hidden()
        .terminal_only()
        .exec(|ctx| {
            match ctx.arg_raw(0)? {
                "error" => bail!("debug error"),
                "panic" => panic!("debug panic"),
                _ => cmd_error!("unknown debugging command"),
            }
        })
];