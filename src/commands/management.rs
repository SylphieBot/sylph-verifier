use super::*;

use log::LogLevelFilter;
use std::process::exit;

const LOG_LEVEL_FILTER_ERROR: &'static str =
    "Could not parse log level. [valid levels: trace, debug, info, warn, error, off]";

pub static COMMANDS: &'static [Command] = &[
    Command::new("shutdown")
        .help(Some(" [--force]"), "Shuts down the bot.")
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
        .help(Some(" <log level> [library log level]"), "Sets the logging level.")
        .terminal_only()
        .exec(|ctx| {
            let app_level = ctx.arg::<LogLevelFilter>(0, LOG_LEVEL_FILTER_ERROR)?;
            let lib_level = ctx.arg_opt::<LogLevelFilter>(1, LOG_LEVEL_FILTER_ERROR)?
                .unwrap_or(app_level);
            ctx.core.set_app_log_level(app_level);
            ctx.core.set_lib_log_level(lib_level);
            Ok(())
        })
];