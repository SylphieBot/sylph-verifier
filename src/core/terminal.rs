use commands::*;
use core::{CoreRef, BotPermission};
use enumset::*;
use error_report;
use errors::*;
use linefeed::{Interface, Terminal as LinefeedTerminal, Signal, ReadResult};
use linefeed::terminal::*;
use logger;
use std::cmp::min;
use std::io;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::*;
use util;

// TODO: Exclude sensitive commands from logging somehow?
// TODO: Remove this horrific terminal hack if built-in support for this is ever added.

type TerminalState = <DefaultTerminal as LinefeedTerminal>::PrepareState;

struct TerminalReaderWrapper<'a>(Box<dyn TerminalReader<DefaultTerminal> + 'a>, Arc<AtomicBool>);
impl <'a> TerminalReader<TerminalWrapper> for TerminalReaderWrapper<'a> {
    fn prepare(
        &mut self, block_signals: bool, report_signals: SignalSet,
    ) -> io::Result<TerminalState> {
        self.0.prepare(block_signals, report_signals)
    }
    unsafe fn prepare_with_lock(
        &mut self, lock: &mut dyn TerminalWriter<TerminalWrapper>,
        block_signals: bool, report_signals: SignalSet,
    ) -> io::Result<TerminalState> {
        self.0.prepare_with_lock(mem::transmute(lock), block_signals, report_signals)
    }
    fn restore(&mut self, state: TerminalState) -> io::Result<()> {
        self.0.restore(state)
    }
    unsafe fn restore_with_lock(
        &mut self, lock: &mut dyn TerminalWriter<TerminalWrapper>,
        state: TerminalState,
    ) -> io::Result<()> {
        self.0.restore_with_lock(mem::transmute(lock), state)
    }

    fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<RawRead> {
        self.wait_for_input(None)?;
        if self.1.load(Ordering::Relaxed) {
            Ok(RawRead::Signal(Signal::Quit))
        } else {
            self.0.read(buf)
        }
    }
    fn wait_for_input(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        let end_time = timeout.map(|duration| Instant::now() + duration);
        let mut now = Instant::now();
        while end_time.map_or(true, |end| now < end) {
            let duration = end_time.map_or(Duration::from_millis(10), |end_time|
                end_time.duration_since(now)
            );
            let duration = min(Duration::from_millis(10), duration);
            if self.0.wait_for_input(Some(duration))? || self.1.load(Ordering::Relaxed) {
                return Ok(true)
            }
            now = Instant::now();
        }
        Ok(false)
    }
}

struct TerminalWrapper(DefaultTerminal, Arc<AtomicBool>);
impl LinefeedTerminal for TerminalWrapper {
    type PrepareState = TerminalState;

    fn name(&self) -> &str {
        self.0.name()
    }

    fn lock_read<'a>(&'a self) -> Box<dyn TerminalReader<Self> + 'a> {
        Box::new(TerminalReaderWrapper(self.0.lock_read(), self.1.clone()))
    }

    fn lock_write<'a>(&'a self) -> Box<dyn TerminalWriter<Self> + 'a> {
        unsafe { mem::transmute(self.0.lock_write()) }
    }
}

struct TerminalContext {
    line: String, command_no: usize,
}
impl CommandContextData for TerminalContext {
    fn permissions(&self) -> EnumSet<BotPermission> {
        EnumSet::all()
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
    fn respond(&self, message: &str) -> Result<()> {
        for line in message.split('\n') {
            info!(target: "$raw", "[Command #{}] {}", self.command_no, line);
        }
        Ok(())
    }
}

crate struct Terminal {
    core_ref: CoreRef, is_interrupted: Arc<AtomicBool>,
}
impl Terminal {
    pub(in ::core) fn new(core_ref: CoreRef) -> Result<Terminal> {
        Ok(Terminal { core_ref, is_interrupted: Arc::new(AtomicBool::new(false)) })
    }
    crate fn open(&self) -> Result<()> {
        let term = TerminalWrapper(DefaultTerminal::new()?, self.is_interrupted.clone());
        let interface = Arc::new(Interface::with_term("sylph-verifier", term)?);
        interface.set_report_signal(Signal::Quit, true);
        interface.set_history_size(1000);
        interface.set_prompt("sylph-verifier> ")?;

        {
            let interface = interface.clone();
            logger::set_log_sender(move |line| {
                write!(interface, "{}\n", line)?;
                Ok(())
            })
        }

        let mut last_line = String::new();
        'outer: loop {
            match interface.read_line() {
                Ok(ReadResult::Input(line)) => {
                    error_report::catch_error(|| {
                        let line = line.trim();

                        info!(target: "$command_input", "{}", line);

                        if line.is_empty() {
                            return Ok(())
                        }

                        if line != last_line {
                            interface.add_history(line.to_owned());
                            last_line = line.to_owned();
                        }

                        let command_no = util::command_id();
                        if let Some(command) = get_command(line) {
                            let ctx = TerminalContext {
                                line: line.to_owned(), command_no,
                            };

                            if command.no_threading {
                                self.core_ref.run_command(command, &ctx)
                            } else {
                                let core_ref = self.core_ref.clone();
                                thread::Builder::new()
                                    .name(format!("command #{}", ctx.command_no + 1))
                                    .spawn(move || core_ref.run_command(command, &ctx))?;
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
                Ok(ReadResult::Signal(Signal::Quit)) => {
                    println!(" (killed)\n");
                    break 'outer;
                }
                Ok(ReadResult::Signal(_)) =>
                    unreachable!(),
                Err(err) =>
                    error!("Reader encountered error: {}", err),
            }
            if !self.core_ref.is_alive() {
                break
            }
        }
        logger::remove_log_sender();
        Ok(())
    }
    crate fn interrupt(&self) {
        self.is_interrupted.store(true, Ordering::Relaxed);
    }
}
