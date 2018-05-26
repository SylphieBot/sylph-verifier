use super::*;

use roblox::*;
use std::process::exit;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

crate const COMMANDS: &[Command] = &[
    Command::new("shutdown")
        .help(Some("[--force]"), "Shuts down the bot.")
        .no_threading()
        .required_permissions(enum_set!(BotPermission::ManageBot))
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
        .required_permissions(enum_set!(BotPermission::ManageVerification))
        .exec(|ctx| {
            ctx.core.verifier().rekey(true)?;
            ctx.core.refresh_place()?;
            Ok(())
        }),

    // Discord management
    Command::new("connect")
        .help(None, "Connects to Discord.")
        .required_permissions(enum_set!(BotPermission::ManageBot))
        .exec(|ctx| {
            ctx.core.discord().connect()
        }),
    Command::new("disconnect")
        .help(None, "Disconnects from Discord.")
        .required_permissions(enum_set!(BotPermission::ManageBot))
        .exec(|ctx| {
            ctx.core.discord().disconnect()
        }),
    Command::new("reconnect")
        .help(None, "Reconnects to Discord.")
        .required_permissions(enum_set!(BotPermission::ManageBot))
        .exec(|ctx| {
            ctx.core.discord().reconnect()
        }),

    // Debugging commands
    Command::new("debug_cmd")
        .hidden()
        .help(Some("<command> [args]"), "")
        .required_permissions(enum_set!(BotPermission::BotAdmin))
        .exec(|ctx| {
            let production = ctx.core.config().get(None, ConfigKeys::ProductionMode)?;
            cmd_ensure!(!production,
                        "Cannot use this command in production mode. If you really want to do \
                         this, execute the following SQL statement on the database, then restart \
                         the server:\n\
                         `INSERT INTO global_config (key, value) \
                          VALUES (\"ProductionMode\", false);`");
            match ctx.arg(0)? {
                "test_error" => bail!("Error triggered by command."),
                "test_panic" => panic!("Panic triggered by command."),
                "test_deadlock" => {
                    let mutex_a1 = Arc::new(Mutex::new(()));
                    let mutex_b1 = Arc::new(Mutex::new(()));
                    let mutex_a2 = mutex_a1.clone();
                    let mutex_b2 = mutex_b1.clone();
                    thread::spawn(move || {
                        let _lock_a = mutex_a1.lock();
                        thread::sleep(Duration::from_secs(1));
                        let _lock_b = mutex_b1.lock();
                    });
                    thread::spawn(move || {
                        let _lock_b = mutex_b2.lock();
                        thread::sleep(Duration::from_secs(1));
                        let _lock_a = mutex_a2.lock();
                    });
                    ctx.respond("Deadlock created.")
                },
                "sleep" => {
                    ::std::thread::sleep(::std::time::Duration::from_secs(3));
                    ctx.respond("Sleep completed.")
                }
                "make_token" => {
                    let username = ctx.arg(1)?;
                    let roblox_id = RobloxUserID::for_username(username)?;
                    ctx.respond(format!("Token for Roblox user {}: {}",
                                        username, ctx.core.verifier().make_token(roblox_id)?))
                },
                _ => cmd_error!("Unknown debug command."),
            }
        })
];