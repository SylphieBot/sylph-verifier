use super::*;
use super::util::*;

macro_rules! config_values {
    ($($perm_name:ident => $perm_token:ident),* $(,)*) => {
        fn perm_to_name(token: BotPermission) -> &'static str {
            match token {
                $(BotPermission::$perm_token => stringify!($perm_name),)*
            }
        }
        fn name_to_perm(name: &str) -> Result<BotPermission> {
            lazy_static! {
                static ref TOKEN: HashMap<&'static str, BotPermission> = {
                    let mut map = HashMap::new();
                    $(map.insert(stringify!($perm_name), BotPermission::$perm_token);)*
                    map
                };
            }
            TOKEN.get(&name).cloned().to_cmd_err(|| format!("No such permission '{}'.", name))
        }
    }
}
config_values! {
    // Bypass permissions
    bot_admin              => BotAdmin,
    server_admin           => GuildAdmin,

    // Global permissions
    manage_bot             => ManageBot,
    manage_global_settings => ManageGlobalSetings,
    manage_verification    => ManageVerification,

    // Guild permissions
    bypass_hierarchy       => BypassHierarchy,
    manage_server_settings => ManageGuildSettings,
    manage_roles           => ManageRoles,

    // Command permissions
    cmd_unverify           => Unverify,
    cmd_unverify_other     => UnverifyOther,
    cmd_whois              => Whois,
    cmd_whowas             => Whowas,

    // Logging permissions
    log_all_verifications  => LogAllVerifications,
}

fn append_perms(buffer: &mut String, perms: EnumSet<BotPermission>) -> Result<()> {
    let mut is_first = true;
    for role in perms {
        if !is_first { write!(buffer, ", ")?; }
        is_first = false;
        write!(buffer, "{}", perm_to_name(role))?;
    }
    Ok(())
}

fn set_perms_for_scope(ctx: &CommandContext, scope: Scope, mut last_arg: usize) -> Result<()> {
    let mut add_perms = EnumSet::new();
    let mut sub_perms = EnumSet::new();

    while let Some(arg) = ctx.arg_opt(last_arg) {
        if arg.starts_with('+') {
            add_perms.insert(name_to_perm(&arg[1..])?);
        } else if arg.starts_with('-') {
            sub_perms.insert(name_to_perm(&arg[1..])?);
        } else {
            cmd_error!("Invalid permission descriptor: {}", arg)
        }
        last_arg += 1;
    }

    let mut scope_perms = ctx.core.permissions().get_scope(scope)?;
    if !add_perms.is_empty() || !sub_perms.is_empty() {
        if !add_perms.is_disjoint(sub_perms) {
            let mut error = String::new();
            write!(error, "Permissions are set to be both removed and added: ")?;
            append_perms(&mut error, add_perms & sub_perms)?;
            cmd_error!(error);
        }
        scope_perms |= add_perms;
        scope_perms -= sub_perms;
        ctx.core.permissions().set_scope(scope, scope_perms)?;
    }

    let mut result = String::new();
    write!(result, "{}", if !add_perms.is_empty() || !sub_perms.is_empty() {
        "New permissions for scope: "
    } else {
        "Permissions for scope: "
    })?;
    append_perms(&mut result, scope_perms)?;
    ctx.respond(&result)
}

fn set_perms(
    ctx: &CommandContext, parse_scope: impl Fn(&str, &str) -> Result<Scope>,
) -> Result<()> {
    let scope_main = ctx.arg(0)?;
    let mut last_arg = 1;
    while let Some(arg) = ctx.arg_opt(last_arg) {
        if arg.starts_with('+') || arg.starts_with('-') { break }
        last_arg += 1;
    }
    let scope_args = if last_arg == 1 { "" } else { ctx.arg_between(1, last_arg - 1)? };
    set_perms_for_scope(ctx, parse_scope(scope_main, scope_args)?, last_arg)
}

crate const COMMANDS: &[Command] = &[
    Command::new("access")
        .help(None, "Shows the permissions you have.")
        .hidden()
        .exec(|ctx| {
            let mut access = String::new();
            write!(access, "You have the following permissions: ")?;
            append_perms(&mut access, ctx.permissions())?;
            ctx.respond(&access)
        }),
    Command::new("set_global_perms")
        .help(Some("<all_servers|all_users|user <user id or mention>> \
                    [+add_perm] [-remove_perm]"),
              "Set global user permissions.")
        .required_permissions(enum_set!(BotPermission::BotAdmin))
        .exec(|ctx| set_perms(ctx, |scope, args| match scope {
            "all_servers" => Ok(Scope::GlobalAllGuilds),
            "all_users" => Ok(Scope::GlobalAllUsers),
            "user" => Ok(Scope::User(find_user(args)?.to_cmd_err(|| "Could not parse user id.")?)),
            x => cmd_error!("Unknown scope: {} {}", x, args),
        })),
    Command::new("set_server_perms")
        .help(Some("[+add_perm] [-remove_perm]"),
              "Set permissions allowed to be set to the current server.")
        .required_permissions(enum_set!(BotPermission::BotAdmin))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg|
            set_perms_for_scope(ctx, Scope::Guild(msg.guild_id()?), 0)
        ),
    Command::new("set_perms")
        .help(Some("<all_users|role <role id or name>|user <user id or mention>> \
                    [+add_perm] [-remove_perm]"),
              "Set user permissions for this server.")
        .required_permissions(enum_set!(BotPermission::GuildAdmin))
        .allowed_contexts(enum_set!(CommandTarget::ServerMessage))
        .exec_discord(|ctx, _, msg| set_perms(ctx, |scope, args| {
            let guild_id = msg.guild_id()?;
            match scope {
                "all_users" => Ok(Scope::GuildAllUsers(guild_id)),
                "role" => Ok(Scope::GuildRole(guild_id, find_role(guild_id, args)?)),
                "user" => {
                    let user_id = find_user(args)?.to_cmd_err(|| "Could not parse user id.")?;
                    Ok(Scope::GuildUser(guild_id, user_id))
                },
                x => cmd_error!("Unknown scope: {} {}", x, args),
            }
        })),
];