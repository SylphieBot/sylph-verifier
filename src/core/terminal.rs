use commands::*;
use core::*;
use error_report;
use errors::*;
use linefeed::*;
use linefeed::reader::LogSender;
use logger;
use parking_lot::Mutex;
use std::io;
use std::thread;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TerminalContext {
    line: String, command_no: usize,
}
impl CommandContextData for TerminalContext {
    fn privilege_level(&self) -> PrivilegeLevel {
        PrivilegeLevel::Terminal
    }
    fn command_target(&self) -> CommandTarget {
        CommandTarget::Terminal
    }

    fn prefix(&self) -> &str {
        ""
    }
    fn message_content(&self) -> &str {
        &self.line
    }
    fn respond(&self, message: &str, mention_user: bool) -> Result<()> {
        for line in message.split("\n") {
            info!(target: "$raw", "[Command #{}] {}", self.command_no, line);
        }
        Ok(())
    }
}

static COMMAND_ID: AtomicUsize = AtomicUsize::new(0);

pub struct Terminal {
    cmd_sender: CommandSender, sender: Mutex<Option<LogSender>>,
}
impl Terminal {
    pub(in ::core) fn new(cmd_sender: CommandSender) -> Result<Terminal> {
        Ok(Terminal { cmd_sender, sender: Mutex::new(None) })
    }
    pub fn start(&self) -> Result<()> {
        let mut reader = Reader::new("sylph-verifier")?;
        reader.set_prompt("sylph-verifier> ");
        reader.set_history_size(1000);

        logger::set_log_sender(reader.get_log_sender());
        *self.sender.lock() = Some(reader.get_log_sender());

        let mut last_line = String::new();
        'outer: loop {
            match reader.read_line() {
                Ok(ReadResult::Input(line)) => {
                    error_report::catch_error(|| {
                        let line = line.trim();

                        info!(target: "$command_input", "{}", line);

                        if line.is_empty() {
                            return Ok(())
                        }

                        if line != last_line {
                            reader.add_history(line.to_owned());
                            last_line = line.to_owned();
                        }

                        let command_no = COMMAND_ID.fetch_add(1, Ordering::Relaxed);
                        if let Some(command) = get_command(&line) {
                            let ctx = TerminalContext {
                                line: line.to_owned(), command_no,
                            };

                            if command.no_threading {
                                self.cmd_sender.run_command(command, &ctx)
                            } else {
                                let cmd_sender = self.cmd_sender.clone();
                                thread::Builder::new()
                                    .name(format!("terminal command #{}", ctx.command_no + 1))
                                    .spawn(move || cmd_sender.run_command(command, &ctx))?;
                                thread::yield_now();
                            }
                        } else {
                            info!(target: "$raw", "[Command #{}] Unknown command.", command_no);
                        }
                        Ok(())
                    }).ok();
                }
                Ok(ReadResult::Eof) =>
                    println!("^D\nPlease use the 'shutdown' command to stop Sylph-Verifier."),
                Ok(ReadResult::Signal(_)) =>
                    unreachable!(),
                Err(err) =>
                    if err.kind() != io::ErrorKind::Interrupted || !reader.was_interrupted() {
                        error!("Reader encountered error: {}", err)
                    } else {
                        break 'outer
                    }
            }
            if !self.cmd_sender.is_alive() {
                break
            }
        }
        for line in reader.stop_log_senders() {
            print!("{}", line);
        }
        logger::remove_log_sender();
        Ok(())
    }
    pub fn interrupt(&self) {
        self.sender.lock().as_ref().map(|x| x.interrupt().ok());
    }
}
