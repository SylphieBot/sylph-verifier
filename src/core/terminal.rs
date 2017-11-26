use core::database::Database;
use core::logger;
use errors::*;
use linefeed::*;

pub fn init(database: &Database) -> Result<!> {
    let mut reader = Reader::new("sylph-verifier")?;
    reader.set_prompt("sylph-verifier> ");
    reader.set_history_size(1000);
    logger::set_log_sender(reader.get_log_sender());

    let mut last_line = String::new();
    loop {
        match reader.read_line() {
            Ok(ReadResult::Input(line)) => {
                if !line.trim().is_empty() && line != last_line {
                    reader.add_history(line.clone());
                    last_line = line.clone();
                }
                info!("Received input: {}", line);
            }
            Ok(ReadResult::Eof) =>
                println!("^D\nPlease use the 'shutdown' command to stop Sylph-Verifier."),
            Ok(ReadResult::Signal(_)) =>
                unreachable!(),
            Err(err) =>
                error!("Reader encountered error: {}", err),
        }
    }
}