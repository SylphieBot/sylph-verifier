use core::*;
use enumset::EnumSet;
use error_report;
use errors::*;
use regex::Regex;
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::collections::HashMap;
use std::fmt::Write;
use std::str::FromStr;

mod util;

enum CommandFn {
    Normal(fn(&CommandContext) -> Result<()>),
    Discord(fn(&CommandContext, &Context, &Message) -> Result<()>),
}
impl CommandFn {
    fn call(&self, ctx: &CommandContext) -> Result<()> {
        match self {
            CommandFn::Normal(f) => f(ctx),
            CommandFn::Discord(f) => match ctx.discord_context() {
                Some((discord_ctx, message)) => f(ctx, discord_ctx, message),
                None => cmd_error!("This command can only be used on Discord."),
            }
        }
    }
}

enum_set_type! {
    pub enum CommandTarget {
        Terminal, ServerMessage, PrivateMessage,
    }
}

pub struct Command {
    name: &'static str, help_args: Option<&'static str>, help_desc: Option<&'static str>,
    allowed_contexts: EnumSet<CommandTarget>, permissions: EnumSet<BotPermission>,
    pub no_threading: bool, hidden: bool, command_fn: Option<CommandFn>,
}
impl Command {
    const fn new(name: &'static str) -> Command {
        Command {
            name, help_args: None, help_desc: None,
            permissions: enum_set!(),
            allowed_contexts: enum_set!(CommandTarget::Terminal |
                                        CommandTarget::ServerMessage |
                                        CommandTarget::PrivateMessage),
            no_threading: false, hidden: false, command_fn: None,
        }
    }
    const fn help(self, args: Option<&'static str>, desc: &'static str) -> Command {
        Command {
            help_args: args, help_desc: Some(desc),
            ..self
        }
    }

    const fn allowed_contexts(self, contexts: EnumSet<CommandTarget>) -> Command {
        Command { allowed_contexts: contexts, ..self }
    }
    const fn hidden(self) -> Command {
        Command { hidden: true, ..self }
    }
    const fn required_permissions(self, permissions: EnumSet<BotPermission>) -> Command {
        Command { permissions, ..self }
    }
    const fn no_threading(self) -> Command {
        Command { no_threading: true, ..self }
    }

    const fn exec(self, f: fn(&CommandContext) -> Result<()>) -> Command {
        Command { command_fn: Some(CommandFn::Normal(f)), ..self }
    }
    const fn exec_discord(
        self, f: fn(&CommandContext, &Context, &Message) -> Result<()>
    ) -> Command {
        Command { command_fn: Some(CommandFn::Discord(f)), ..self }
    }

    pub fn run(&self, ctx: &dyn CommandContextData, core: &VerifierCore) {
        let args = Args::new(ctx.message_content());

        let ctx = CommandContext::new(core, ctx, args, self);
        ctx.catch_error(|| {
            cmd_ensure!(ctx.has_permissions(self.permissions),
                        "You do not have the necessary permissions to use that command.");
            if !self.allowed_contexts.contains(ctx.command_target) {
                match ctx.command_target {
                    CommandTarget::Terminal =>
                        cmd_error!("This command cannot be used in the terminal."),
                    CommandTarget::ServerMessage =>
                        cmd_error!("This command cannot be used from Discord servers."),
                    CommandTarget::PrivateMessage =>
                        cmd_error!("This command cannot be used in DMs."),
                };
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
            sorted_command_names.into_iter().map(|x| commands[&x]).collect();
        CommandList { commands, sorted_commands }
    }

    fn command_list(&self) -> &[&'static Command] {
        &self.sorted_commands
    }
    fn get(&self, command: &str) -> Option<&'static Command> {
        self.commands.get(&command).cloned()
    }
}

lazy_static! {
    static ref ARG_REGEX: Regex = Regex::new(r"\S+").unwrap();
}

struct Args<'a> {
    str: &'a str, matches: Vec<(usize, usize)>,
}
impl <'a> Args<'a> {
    fn new(message: &'a str) -> Args<'a> {
        let mut matches = ARG_REGEX.find_iter(message);
        matches.next(); // Discard command
        Args {
            str: message, matches: matches.map(|x| (x.start(), x.end())).collect(),
        }
    }
}

struct CommandContext<'a> {
    core: &'a VerifierCore,
    permissions: EnumSet<BotPermission>,
    command_target: CommandTarget,
    command: &'a Command,
    data: &'a dyn CommandContextData,
    args: Args<'a>,
}
impl <'a> CommandContext<'a> {
    fn new(core: &'a VerifierCore, data: &'a dyn CommandContextData,
           args: Args<'a>, command: &'a Command) -> CommandContext<'a> {
        CommandContext {
            core, data, args, command,
            permissions: data.permissions(), command_target: data.command_target(),
        }
    }

    fn prefix(&self) -> &str {
        self.data.prefix()
    }
    fn respond(&self, message: impl AsRef<str>) -> Result<()> {
        self.data.respond(message.as_ref().trim())
    }
    fn discord_context(&self) -> Option<(&Context, &Message)> {
        self.data.discord_context()
    }

    fn catch_error<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        match error_report::catch_error(|| match f() {
            Ok(v) => Ok(Ok(v)),
            Err(match_err!(ErrorKind::CommandError(err))) => {
                self.respond(&err)?;
                Ok(Err(ErrorKind::CommandError(err).into()))
            }
            Err(e @ match_err!(ErrorKind::SerenityPermissionError)) => {
                self.respond(
                    "The bot has encountered an unknown permissions error. Please check that:\n\
                     • It has the permissions it requires: Manage Roles, Manage Nicknames, \
                       Read Messages, Send Messages, Manage Messages, Read Message History\n\
                     • There is no per-channel permissions overwrites preventing it from using \
                       those permissions on this channel.\n\
                     • It has a role with a greater rank than all roles it needs to manage."
                )?;
                Ok(Err(e))
            }
            Err(e @ match_err!(ErrorKind::SerenityNotFoundError)) => {
                self.respond(
                    "A user, message, role or channel the bot is configured to use has \
                     been deleted."
                )?;
                Ok(Err(e))
            }
            Err(e) => {
                self.respond("The command encountered an unexpected error. \
                              Please contact the bot owner.")?;
                error!("Command encountered an unexpected error!");
                Err(e)
            }
        }) {
            Ok(Ok(v)) => Ok(v),
            Err(e @ match_err!(ErrorKind::Panicked)) => {
                self.respond("The command encountered an unexpected error. \
                              Please contact the bot owner.")?;
                error!("Command encountered an unexpected error!");
                Err(e)
            }
            Err(e) | Ok(Err(e)) => Err(e),
        }
    }

    fn get_guild(&self) -> Result<Option<GuildId>> {
        match self.data.discord_context() {
            Some((_, message)) =>
                match message.channel()? {
                    Channel::Guild(ch) => Ok(Some(ch.read().guild_id)),
                    Channel::Group(_) | Channel::Private(_) | Channel::Category(_) => Ok(None),
                },
            None => Ok(None),
        }
    }
    fn permissions(&self) -> EnumSet<BotPermission> {
        self.permissions
    }
    fn has_permissions(&self, perms: EnumSet<BotPermission>) -> bool {
        self.permissions.is_superset(perms)
    }

    fn not_enough_arguments(&self) -> String {
        format!("Not enough arguments for command. Usage: {}{}{}",
                self.prefix(), self.command.name,
                self.command.help_args.map_or("".to_owned(), |x| format!(" {}", x)))
    }

    fn argc(&self) -> usize {
        self.args.matches.len()
    }
    fn arg_opt(&self, i: usize) -> Option<&str> {
        if i < self.argc() {
            let arg = self.args.matches[i];
            Some(&self.args.str[arg.0..arg.1])
        } else {
            None
        }
    }
    fn arg(&self, i: usize) -> Result<&str> {
        self.arg_opt(i).to_cmd_err(|| self.not_enough_arguments())
    }

    fn arg_between_opt(&self, first: usize, last: usize) -> Option<&str> {
        if first < self.argc() && last < self.argc() {
            assert!(first <= last);
            Some(&self.args.str[self.args.matches[first].0..self.args.matches[last].1])
        } else {
            None
        }
    }
    fn arg_between(&self, first: usize, last: usize) -> Result<&str> {
        self.arg_between_opt(first, last).to_cmd_err(|| self.not_enough_arguments())
    }

    fn rest_opt(&self, i: usize) -> Option<&str> {
        if i < self.argc() {
            Some(self.args.str[self.args.matches[i].0..].trim())
        } else if i == self.argc() {
            Some("")
        } else {
            None
        }
    }
    fn rest(&self, i: usize) -> Result<&str> {
        self.rest_opt(i).to_cmd_err(|| self.not_enough_arguments())
    }
}

pub trait CommandContextData {
    fn permissions(&self) -> EnumSet<BotPermission>;
    fn command_target(&self) -> CommandTarget;

    fn prefix(&self) -> &str;
    fn message_content(&self) -> &str;
    fn respond(&self, message: &str) -> Result<()>;

    fn discord_context(&self) -> Option<(&Context, &Message)> { None }
}

mod config;
mod management;
mod permissions;
mod verifier;

static CORE_COMMANDS: &'static [Command] = &[
    Command::new("help")
        .help(Some("<command>"),
              "Lists all available commands, or information on a particular command..")
        .exec(|ctx| {
            if let Some(command) = ctx.arg_opt(0) {
                if let Some(command) = COMMANDS.get(command) {
                    ctx.respond(format!(
                        "{}{}{}{}",
                        ctx.prefix(), command.name,
                        command.help_args.map_or("".to_owned(), |x| format!(" {}", x)),
                        command.help_desc.map_or("".to_owned(), |x| format!(" - {}", x))
                    ))
                } else {
                    ctx.respond("No such command was found.")
                }
            } else {
                let mut buffer = String::new();
                writeln!(buffer, "Command list: ([optional parameter], <required parameter>)")?;
                for command in COMMANDS.command_list() {
                    if !command.hidden &&
                       command.allowed_contexts.contains(ctx.command_target) &&
                       ctx.has_permissions(command.permissions) {
                        writeln!(buffer, "• {}{}{}",
                                 ctx.prefix(), command.name,
                                 command.help_args.map_or("".to_owned(), |x| format!(" {}", x)))?;
                    }
                }
                writeln!(buffer, "For more information on a command, use `~help command`.")?;
                ctx.respond(buffer)
            }
        })
];
lazy_static! {
    static ref COMMANDS: CommandList = CommandList::new(&[
        CORE_COMMANDS, config::COMMANDS, management::COMMANDS,
        permissions::COMMANDS, verifier::COMMANDS,
    ]);
}
pub fn get_command(msg: &str) -> Option<&'static Command> {
    ARG_REGEX.find(msg).and_then(|c| COMMANDS.get(&c.as_str().to_lowercase()))
}
