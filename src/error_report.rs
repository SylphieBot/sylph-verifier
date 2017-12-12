use backtrace::Backtrace;
use chrono::Utc;
use error_chain::ChainedError;
use errors::*;
use logger;
use parking_lot::RwLock;
use parking_lot::deadlock::check_deadlock;
use std::any::Any;
use std::borrow::Cow;
use std::fmt::{Write as FmtWrite};
use std::fs;
use std::fs::File;
use std::io::{Write as IoWrite};
use std::panic::*;
use std::path::{Path, PathBuf};
use std::process::abort;
use std::thread;
use std::time::Duration;

fn thread_name() -> String {
    thread::current().name().or(Some("<unknown>")).unwrap().to_string()
}
fn cause_from_error<E: ChainedError>(e: &E) -> Result<String> {
    let mut buf = String::new();
    writeln!(buf, "Thread {} errored with '{}'", thread_name(), e)?;
    for e in e.iter().skip(1) {
        writeln!(buf, "Caused by: {}", e)?;
    }
    Ok(buf.trim().to_string())
}
fn cause_from_panic(info: &(Any + Send), loc: Option<&Location>) -> String {
    let raw_cause: Cow<'static, str> = if let Some(&s) = info.downcast_ref::<&str>() {
        format!("'{}'", s).into()
    } else if let Some(s) = info.downcast_ref::<String>() {
        format!("'{}'", s).into()
    } else {
        "unknown panic information".into()
    };
    let raw_location: Cow<'static, str> = loc.map_or("".into(), |loc| {
        format!(" at {}:{}", loc.file(), loc.line()).into()
    });
    format!("Thread '{}' panicked with {}{}", thread_name(), raw_cause, raw_location)
}

fn make_error_report(kind: &str, cause: &str, backtrace: &str) -> Result<String> {
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

fn write_report_file<P: AsRef<Path>>(root_path: P, kind: &str, report: &str) -> Result<PathBuf> {
    let mut path = PathBuf::from(root_path.as_ref());
    path.push("logs");
    fs::create_dir_all(&path)?;
    let file_name = format!("{}_report_{}.log", kind, Utc::now().format("%Y%m%d_%H%M%S%f"));
    path.push(file_name);
    let mut out = File::create(&path)?;
    out.write_all(report.as_bytes())?;
    Ok(path)
}

static ROOT_PATH: RwLock<Option<PathBuf>> = RwLock::new(None);
fn write_report(kind: &str, cause: &str, backtrace: &str, short_cause: bool) -> Result<()> {

    for line in cause.split("\n") {
        error!("{}", line);
        if short_cause {
            break
        }
    }

    let lc_kind = kind.to_lowercase();
    let root_path = ROOT_PATH.read().as_ref().unwrap().clone();
    let report_file = write_report_file(root_path, &lc_kind,
                                        &make_error_report(kind, cause, backtrace)?)?;
    error!("Detailed information about this {} can be found at '{}'.",
           lc_kind, report_file.display());
    Ok(())
}

fn report_err<E: ChainedError>(e: &E) -> Result<()> {
    let cause = cause_from_error(e)?;
    let backtrace = match e.backtrace() {
        Some(bt) => format!("{:?}", bt),
        None => format!("(from catch site)\n{:?}", Backtrace::new()),
    };
    write_report("Error", &cause, &backtrace, false)?;
    Ok(())
}

pub fn init<P: AsRef<Path>>(root_path: P) {
    *ROOT_PATH.write() = Some(root_path.as_ref().to_owned());

    set_hook(Box::new(|panic_info| {
        let cause = cause_from_panic(panic_info.payload(), panic_info.location());
        let backtrace = format!("{:?}", Backtrace::new());
        write_report("Panic", &cause, &backtrace, false).expect("failed to write panic report!");
    }));
}

pub fn catch_error<F, T>(f: F) -> Result<T> where F: FnOnce() -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(t)) => Ok(t),
        Ok(Err(e)) => {
            report_err(&e)?;
            Err(e)
        }
        Err(_) => bail!(ErrorKind::Panicked),
    }
}