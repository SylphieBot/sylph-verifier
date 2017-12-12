use super::*;

use log::LogLevelFilter;
use logger;
use std::process::exit;

const LOG_LEVEL_FILTER_ERROR: &'static str =
    "Could not parse log level. [valid levels: trace, debug, info, warn, error, off]";

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
    prefix<String>(CommandPrefix, true, "The prefix used before commands.",
                   |x| Ok(x.to_owned()), |x| Ok(x), |_| Ok(()));
    token<Option<String>>(DiscordToken, false, "The bot token used to connect to Discord.",
                          |x| Ok(Some(x.to_owned())),
                          |x| Ok(x.map_or("(not set)", |_| "<token redacted>").to_owned()),
                          |_| Ok(()));
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
                writeln!(config, "• {} = {} ({})",
                         option.name, (option.get_config)(ctx, guild)?, option.help)?;
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
                    ctx, guild, if rest.is_empty() { None } else { Some(&rest) }
                )
            }
            None => cmd_error!("No such configuration option '{}'.", config),
        }
    }
}

pub const COMMANDS: &'static [Command] = &[
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
    Command::new("set_log_level")
        .help(Some("<log level> [library log level]"), "Sets the logging level.")
        .terminal_only()
        .exec(|ctx| {
            let app_level = ctx.arg::<LogLevelFilter>(0, LOG_LEVEL_FILTER_ERROR)?;
            let lib_level = ctx.arg_opt::<LogLevelFilter>(1, LOG_LEVEL_FILTER_ERROR)?
                .unwrap_or(app_level);
            logger::set_filter_level(app_level, lib_level);
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
];