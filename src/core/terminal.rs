use commands::*;
use core::VerifierCore;
use error_report;
use errors::*;
use linefeed::*;
use linefeed::reader::LogSender;
use logger;
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

pub struct Terminal<'a> {
    core: &'a VerifierCore,
    reader: Reader<DefaultTerminal>,
}
impl <'a> Terminal<'a> {
    pub fn new(core: &VerifierCore) -> Result<Terminal> {
        let mut reader = Reader::new("sylph-verifier")?;
        reader.set_prompt("sylph-verifier> ");
        reader.set_history_size(1000);
        logger::set_log_sender(reader.get_log_sender());
        Ok(Terminal {
            core, reader,
        })
    }
    pub fn new_sender(&mut self) -> LogSender {
        self.reader.get_log_sender()
    }
    pub fn start(&mut self) -> Result<()> {
        let mut last_line = String::new();
        'outer: loop {
            match self.reader.read_line() {
                Ok(ReadResult::Input(line)) => {
                    error_report::catch_error(|| {
                        let line = line.trim();

                        info!(target: "$command_input", "{}", line);

                        if line.is_empty() {
                            return Ok(())
                        }

                        if line != last_line {
                            self.reader.add_history(line.to_owned());
                            last_line = line.to_owned();
                        }

                        let command_no = COMMAND_ID.fetch_add(1, Ordering::Relaxed);
                        if let Some(command) = get_command(&line) {
                            let ctx = TerminalContext {
                                line: line.to_owned(), command_no,
                            };
                            let core = self.core.clone();

                            if command.no_threading {
                                command.run(&ctx, &core)
                            } else {
                                thread::Builder::new()
                                    .name(format!("terminal command #{}", ctx.command_no + 1))
                                    .spawn(move || command.run(&ctx, &core))?;
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
                    if err.kind() != io::ErrorKind::Interrupted || !self.reader.was_interrupted() {
                        error!("Reader encountered error: {}", err)
                    } else {
                        break 'outer
                    }
            }
            if !self.core.is_alive() {
                break
            }
        }
        for line in self.reader.stop_log_senders() {
            print!("{}", line);
        }
        Ok(())
    }
}
