use error_chain::{Backtrace, ChainedError};
use std;
use std::fmt::Write;
use rusqlite;

error_chain! {
    foreign_links {
        Fmt(std::fmt::Error);
        Io(std::io::Error);
        StringFromUtf8Error(std::string::FromUtf8Error);
        SystemTimeError(std::time::SystemTimeError);
        Rusqlite(rusqlite::Error);
    }

    errors {
        InvalidToken {
            description("token must be six upper case letters")
        }

        LZ4Error {
            description("LZ4 error")
        }

        RblxWrongHeader {
            description("place file had invalid header")
        }

        RblxWrongFooter {
            description("place file had invalid footer")
        }

        NonZeroUnknownField {
            description("unknown field in place file not zero")
        }

        UnknownTypeID(v: u32) {
            description("found unknown type id")
            display("found unknown type id: {}", v)
        }

        WrongPlaceVersion {
            description("wrong place version")
        }

        UnknownDatabaseVersion {
            description("unknown database version")
        }
    }
}

fn make_backtrace_str(backtrace: Option<&Backtrace>) -> String {
    if let Some(backtrace) = backtrace {
        format!("(backtrace from error)\n{:?}", backtrace)
    } else {
        format!("(current backtrace)\n{:?}", Backtrace::new())
    }
}
fn make_error_report(cause: &str, backtrace: &str) -> Result<String> {
    let mut buf = String::new();
    writeln!(buf, "--- Sylph-Verifier Error Report ---")?;
    writeln!(buf)?;
    writeln!(buf, "Version: {} {} ({}{}{})",
                  env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), env!("TARGET"),
                  if env!("TARGET") != env!("HOST") {
                      format!(", cross-compiled from {}", env!("HOST"))
                  } else { "".to_owned() },
                  if env!("PROFILE") != "release" {
                      format!(", {}", env!("PROFILE"))
                  } else { "".to_owned() })?;
    writeln!(buf, "Commit: {}{}",
                  env!("GIT_COMMIT"),
                  if option_env!("GIT_IS_DIRTY").is_some() { " (dirty)" } else { "" })?;
    writeln!(buf)?;
    writeln!(buf, "{}", cause.trim())?;
    writeln!(buf)?;
    writeln!(buf, "{}", backtrace)?;
    Ok(buf)
}

fn cause_from_error<E: ChainedError>(e: &E) -> Result<String> {
    let mut buf = String::new();
    writeln!(buf, "Error: {}", e)?;
    for e in e.iter().skip(1) {
        writeln!(buf, "Caused by: {}", e)?;
    }
    Ok(buf)
}
fn report_from_error<E: ChainedError>(e: &E) -> Result<String> {
    let cause = cause_from_error(e)?;
    let backtrace = make_backtrace_str(e.backtrace());
    make_error_report(&cause, &backtrace)
}

pub fn error_report_test() {
    ::std::env::set_var("RUST_BACKTRACE", "1");
    println!("{}", report_from_error(&Error::from_kind(ErrorKind::WrongPlaceVersion)
        .chain_err(|| ErrorKind::WrongPlaceVersion)
        .chain_err(|| ErrorKind::WrongPlaceVersion)
        .chain_err(|| ErrorKind::WrongPlaceVersion)).unwrap());
}