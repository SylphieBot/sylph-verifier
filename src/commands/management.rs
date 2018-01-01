use super::*;

use std::process::exit;

pub const COMMANDS: &[Command] = &[
    Command::new("shutdown")
        .help(Some("[--force]"), "Shuts down the bot.")
        .no_threading()
        .terminal_only()
        .exec(|ctx| {
            match ctx.arg_opt(0) {
                Some("--force") => { exit(1); }
                _ => { ctx.core.shutdown().ok(); }
            }
            Ok(())
        }),

    // Configuration
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
        .required_privilege(PrivilegeLevel::BotOwner)
        .exec(|ctx| {
            match ctx.arg(0)? {
                "test_error" => bail!("Error triggered by command."),
                "test_panic" => panic!("Panic triggered by command."),
                _ => cmd_error!("Unknown debug command."),
            }
        })
];