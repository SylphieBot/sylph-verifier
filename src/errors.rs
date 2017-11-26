#![allow(non_camel_case_types)]

use chrono::Utc;
use diesel;
use error_chain::{Backtrace, ChainedError};
use r2d2;
use reqwest;
use serde_json;
use std;
use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt::{Write as FmtWrite};
use std::fs;
use std::fs::File;
use std::io::{Write as IoWrite};
use std::panic::*;
use std::path::PathBuf;

error_chain! {
    foreign_links {
        Diesel(diesel::result::Error);
        Diesel_ConnectionError(diesel::result::ConnectionError);
        Fmt(std::fmt::Error);
        Io(std::io::Error);
        R2D2_GetTimeout(r2d2::GetTimeout);
        R2D2_InitializationError(r2d2::InitializationError);
        R2D2_RunMigrationsError(diesel::migrations::RunMigrationsError);
        Reqwest(reqwest::Error);
        SerdeJson(serde_json::Error);
        Str_Utf8Error(std::str::Utf8Error);
        String_FromUtf8Error(std::string::FromUtf8Error);
        SystemTimeError(std::time::SystemTimeError);
    }

    errors {
        InvalidToken {
            description("token must be six upper case letters")
        }

        LZ4Error {
            description("LZ4 error")
        }

        Panicked {
            description("panic encountered")
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
fn make_error_report(cause: &str, backtrace: &str, kind: &str) -> Result<String> {
    let mut buf = String::new();
    writeln!(buf, "--- Sylph-Verifier {} Report ---", kind)?;
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
fn cause_from_panic(info: &(Any + Send)) -> Cow<'static, str> {
    if let Some(&s) = info.downcast_ref::<&str>() {
        Cow::from(s.to_string())
    } else if let Some(s) = info.downcast_ref::<String>() {
        Cow::from(s.clone())
    } else {
        Cow::from("Panicked: <could not get panic error>")
    }
}

struct ParsedPanicLocation {
    file: String, line: u32,
}
struct ParsedPanicInfo {
    cause_str: Cow<'static, str>, location: Option<ParsedPanicLocation>,
}
enum PanicInfoStatus {
    NoPanic, PanicReady, Error, PanicInfo(ParsedPanicInfo),
}
thread_local! {
    static PANIC_INFO: RefCell<PanicInfoStatus> = RefCell::new(PanicInfoStatus::NoPanic);
}

fn set_info(status: PanicInfoStatus) {
    PANIC_INFO.with(|info| {
        let mut info = info.borrow_mut();
        *info = status;
    })
}
pub fn init_panic_hook() {
    let default_hook = take_hook();
    set_hook(Box::new(move |panic_info| {
        PANIC_INFO.with(|info| {
            if let &PanicInfoStatus::NoPanic = &*info.borrow() {
                default_hook(panic_info)
            } else if let &PanicInfoStatus::Error = &*info.borrow() {
                // Ignored
            } else {
                set_info(PanicInfoStatus::PanicInfo(ParsedPanicInfo {
                    cause_str: cause_from_panic(panic_info.payload()),
                    location: panic_info.location().map(|location| ParsedPanicLocation {
                        file: location.file().to_owned(), line: location.line(),
                    })
                }))
            }
        })
    }))
}

fn write_error_report(kind: &str, report: &str) -> Result<PathBuf> {
    let mut path = PathBuf::from("logs") /* TODO Fix */;
    fs::create_dir_all(&path)?;
    let file_name = format!("{}_report_{}.log", kind, Utc::now().format("%Y%m%d_%H%M%S%f"));
    path.push(file_name);
    let mut out = File::create(&path)?;
    out.write_all(report.as_bytes())?;
    Ok(path)
}

fn report_from_error<E: ChainedError>(e: &E) -> Result<String> {
    let cause = cause_from_error(e)?;
    let backtrace = make_backtrace_str(e.backtrace());
    make_error_report(&cause, &backtrace, "Error")
}
fn report_from_panic(info: &(Any + Send)) -> Result<String> {
    let cause = cause_from_panic(info);
    let backtrace = make_backtrace_str(None);
    make_error_report(cause.as_ref(), &backtrace, "Panic")
}
