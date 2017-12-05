use core::*;
use enumset::EnumSet;
use errors::*;
use regex::Regex;
use serenity::model::*;
use serenity::prelude::*;
use std::collections::HashMap;
use std::str::FromStr;

enum CommandFn {
    Normal(fn(&CommandContext) -> Result<()>),
    Discord(fn(&CommandContext, &Context, &Message) -> Result<()>),
}
impl CommandFn {
    fn call(&self, ctx: &CommandContext) -> Result<()> {
        match self {
            &CommandFn::Normal(f) => f(ctx),
            &CommandFn::Discord(f) => match ctx.discord_context() {
                Some((discord_ctx, message)) => f(ctx, discord_ctx, message),
                None => ctx.error("This command can only be used on Discord."),
            }
        }
    }
}

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum PrivilegeLevel {
    NormalUser, GuildOwner, BotOwner, Terminal,
}

enum_set_type! {
    pub enum CommandTarget {
        Terminal, ServerMessage, PrivateMessage, Unknown,
    }
}

pub struct CooldownInfo {
    pub ident: &'static str, pub message: &'static str,
    pub max_attempts: u32, pub cooldown_secs: u32, pub database_backed: bool,
}

pub struct Command {
    name: &'static str, help_args: Option<&'static str>, help_desc: Option<&'static str>,
    required_privilege: PrivilegeLevel, allowed_contexts: EnumSet<CommandTarget>,
    discord_permissions: Option<Permissions>, pub no_threading: bool,command_fn: Option<CommandFn>,
}
impl Command {
    pub(self) const fn new(name: &'static str) -> Command {
        Command {
            name, help_args: None, help_desc: None,
            required_privilege: PrivilegeLevel::NormalUser,
            discord_permissions: None,
            allowed_contexts: enum_set!(CommandTarget::Terminal |
                                        CommandTarget::ServerMessage |
                                        CommandTarget::PrivateMessage),
            no_threading: false, command_fn: None,
        }
    }
    pub(self) const fn help(self, args: Option<&'static str>, desc: &'static str) -> Command {
        Command {
            help_args: args, help_desc: Some(desc),
            ..self
        }
    }

    pub(self) const fn required_privilege(self, privilege: PrivilegeLevel) -> Command {
        Command { required_privilege: privilege, ..self }
    }
    pub(self) const fn allowed_contexts(self, contexts: EnumSet<CommandTarget>) -> Command {
        Command { allowed_contexts: contexts, ..self }
    }
    pub(self) const fn terminal_only(self) -> Command {
        Command {
            allowed_contexts: enum_set!(CommandTarget::Terminal),
            required_privilege: PrivilegeLevel::Terminal,
            ..self
        }
    }
    pub(self) const fn no_threading(self) -> Command {
        Command { no_threading: true, ..self }
    }

    pub(self) const fn exec(self, f: fn(&CommandContext) -> Result<()>) -> Command {
        Command { command_fn: Some(CommandFn::Normal(f)), ..self }
    }
    pub(self) const fn exec_discord(self, f: fn(&CommandContext,
                                          &Context, &Message) -> Result<()>) -> Command {
        Command { command_fn: Some(CommandFn::Discord(f)), ..self }
    }

    pub fn run(&self, ctx: &CommandContextData, core: &VerifierCore) {
        let args = Args::new(ctx.message_content());

        let ctx = CommandContext::new(core, ctx, args);
        ctx.catch_error(|| {
            if ctx.privilege_level < self.required_privilege {
                ctx.error("You do not have the necessary permissions to use \
                           that command.")?;
            }
            if !self.allowed_contexts.contains(ctx.command_target) {
                let err_str = match ctx.command_target {
                    CommandTarget::Terminal =>
                        "This command cannot be used in the terminal.",
                    CommandTarget::ServerMessage =>
                        "This command cannot be used from Discord servers.",
                    CommandTarget::PrivateMessage =>
                        "This command cannot be used in DMs.",
                    CommandTarget::Unknown =>
                        "This command cannot be used here.",
                };
                ctx.error(err_str)?;
            }
            self.command_fn.as_ref().unwrap().call(&ctx)
        }).ok();
    }
}

struct CommandList {
    sorted_commands: Vec<&'static Command>,
    commands: HashMap<&'static str, &'static Command>,
}
impl CommandList {
    fn new(lists: &[&'static [Command]]) -> CommandList {
        let mut commands = HashMap::new();
        let mut sorted_command_names = Vec::new();
        for &list in lists {
            for command in list {
                if commands.contains_key(&command.name) {
                    panic!("Duplicate command '{}'", command.name)
                }
                if command.command_fn.is_none() {
                    panic!("Command '{}' has no implementation!", command.name)
                }
                sorted_command_names.push(command.name);
                commands.insert(command.name, command);
            }
        }
        sorted_command_names.sort();
        let sorted_commands =
            sorted_command_names.into_iter().map(|x| *commands.get(&x).unwrap()).collect();
        CommandList { commands, sorted_commands }
    }

    fn command_list(&self) -> &[&'static Command] {
        &self.sorted_commands
    }
    fn get(&self, command: &str) -> Option<&'static Command> {
        self.commands.get(&command).cloned()
    }
}

struct Args<'a> {
    str: &'a str, command_match: Option<&'a str>, matches: Vec<(usize, usize)>,
}
impl <'a> Args<'a> {
    fn new(message: &'a str) -> Args<'a> {
        lazy_static! {
            static ref REGEX: Regex = Regex::new(r"\S+").unwrap();
        }
        let mut matches = REGEX.find_iter(message);
        let command_match = matches.next().map(|x| x.as_str());
        Args {
            str: message, command_match, matches: matches.map(|x| (x.start(), x.end())).collect(),
        }
    }
}

struct CommandContext<'a> {
    pub core: &'a VerifierCore,
    pub privilege_level: PrivilegeLevel,
    pub command_target: CommandTarget,
    data: &'a CommandContextData,
    args: Args<'a>,
}
impl <'a> CommandContext<'a> {
    fn new(core: &'a VerifierCore, data: &'a CommandContextData,
           args: Args<'a>) -> CommandContext<'a> {
        CommandContext {
            core, data, args,
            privilege_level: data.privilege_level(), command_target: data.command_target(),
        }
    }

    pub fn prefix(&self) -> &str {
        self.data.prefix()
    }
    pub fn respond<S: AsRef<str>>(&self, message: S) -> Result<()> {
        self.data.respond(message.as_ref(), true)
    }
    pub fn respond_raw<S: AsRef<str>>(&self, message: S) -> Result<()> {
        self.data.respond(message.as_ref(), false)
    }
    pub fn discord_context(&self) -> Option<(&Context, &Message)> {
        self.data.discord_context()
    }

    pub fn error<S: AsRef<str>, T>(&self, s: S) -> Result<T> {
        self.respond(s)?;
        bail!(ErrorKind::CommandAborted)
    }

    fn report_error<T>(&self, r: Result<T>) -> Result<T> {
        match &r {
            &Err(_) => {
                self.respond("The command encountered an unknown error. \
                              Please contact the bot owner.").ok(); // is an error anyway
            }
            _ => { }
        }
        r
    }
    pub fn catch_panic<F, R>(&self, f: F) -> Result<R> where F: FnOnce() -> R {
        self.report_error(self.core.catch_panic(f))
    }
    pub fn catch_error<F, T>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.report_error(self.core.catch_error(|| -> Result<Result<T>> {
            match f() {
                Err(Error(box (ErrorKind::CommandAborted, _))) => bail!(ErrorKind::CommandAborted),
                x => Ok(x)
            }
        })?)
    }

    pub fn ensure<S: AsRef<str>>(&self, condition: bool, s: S) -> Result<()> {
        if !condition {
            self.error(s)
        } else {
            Ok(())
        }
    }
    pub fn ensure_option<T, S: AsRef<str>>(&self, o: Option<T>, s: S) -> Result<T> {
        match o {
            Some(t) => Ok(t),
            None => self.error(s),
        }
    }
    pub fn ensure_result<T, S: AsRef<str>>(&self, r: Result<T>, s: S) -> Result<T> {
        match r {
            Ok(t) => Ok(t),
            Err(_) => self.error(s),
        }
    }

    pub fn ensure_permissions(&self, perms: Permissions) -> Result<()> {
        let status =
            self.privilege_level >= PrivilegeLevel::GuildOwner ||
            match self.data.discord_context() {
                Some((ctx, message)) =>
                    match message.channel().chain_err(|| "Failed to get channel.")? {
                        Channel::Guild(ch) =>
                            ch.read().permissions_for(&message.author)?.contains(perms),
                        Channel::Group(_) | Channel::Private(_) | Channel::Category(_) => false,
                    },
                None => false,
            };
        self.ensure(status, "You do not have sufficient permissions to access this command.")
    }

    pub fn min_args(&self, min: usize) -> Result<()> {
        self.ensure(self.argc() >= min,
                    format!("Not enough arguments for command. ({} required)", self.argc()))
    }

    pub fn argc(&self) -> usize {
        self.args.matches.len()
    }
    pub fn arg_opt_raw(&self, i: usize) -> Option<&str> {
        if i < self.argc() {
            let arg = self.args.matches[i];
            Some(&self.args.str[arg.0..arg.1])
        } else {
            None
        }
    }
    pub fn arg_opt<T: FromStr>(&self, i: usize, parse_err: &str) -> Result<Option<T>> {
        match self.arg_opt_raw(i) {
            Some(t) => match t.parse::<T>() {
                Ok(t) => Ok(Some(t)),
                Err(_) => self.error(format!("Could not parse argument #{}", i + 1)),
            },
            None => Ok(None),
        }
    }
    pub fn arg_raw(&self, i: usize) -> Result<&str> {
        self.ensure_option(self.arg_opt_raw(i), "Not enough arguments for command.")
    }
    pub fn arg<T: FromStr>(&self, i: usize, parse_err: &str) -> Result<T> {
       self.ensure_option(self.arg_opt(i, parse_err)?, "Not enough arguments for command.")
    }

    pub fn rest_opt(&self, i: usize) -> Option<&str> {
        if i < self.argc() {
            Some(&self.args.str[self.args.matches[i].0..].trim())
        } else {
            None
        }
    }
    pub fn rest(&self, i: usize) -> Result<&str> {
        self.ensure_option(self.rest_opt(i), "Not enough arguments for command.")
    }
}

pub trait CommandContextData {
    fn privilege_level(&self) -> PrivilegeLevel;
    fn command_target(&self) -> CommandTarget;

    fn prefix(&self) -> &str;
    fn message_content(&self) -> &str;
    fn respond(&self, message: &str, mention_user: bool) -> Result<()>;

    fn discord_context(&self) -> Option<(&Context, &Message)> { None }
}

mod management;

static CORE_COMMANDS: &'static [Command] = &[
    Command::new("help")
        .help(None, "Lists all available commands.")
        .exec(|ctx| {
            ctx.respond("Command list: ([optional parameter], <required parameter>)")?;
            for command in COMMANDS.command_list() {
                if ctx.privilege_level >= command.required_privilege &&
                   command.allowed_contexts.contains(ctx.command_target) {
                    ctx.respond(format!("• {}{}{}{}",
                                        ctx.prefix(), command.name,
                                        command.help_args.map_or("".to_owned(),
                                                                 |x| format!(" {}", x)),
                                        command.help_desc.map_or("".to_owned(),
                                                                 |x| format!(" - {}", x))))?;
                }
            }
            Ok(())
        })
];
lazy_static! {
    static ref COMMANDS: CommandList = CommandList::new(&[
        CORE_COMMANDS, management::COMMANDS,
    ]);
}
pub fn get_command(msg: &str) -> Option<&'static Command> {
    let args = Args::new(msg);
    args.command_match.and_then(|c| COMMANDS.get(&c.to_lowercase()))
}
