use chrono::Utc;
use error_chain::{Backtrace, ChainedError};
use errors::*;
use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt::{Write as FmtWrite};
use std::fs;
use std::fs::File;
use std::io::{Write as IoWrite};
use std::mem::replace;
use std::panic::*;
use std::path::{Path, PathBuf};
use std::process::abort;
use std::thread;

fn make_backtrace_str(backtrace: Option<&Backtrace>) -> String {
    if let Some(backtrace) = backtrace {
        format!("{:?}", backtrace)
    } else {
        format!("{:?}", Backtrace::new())
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

fn cause_from_error<E: ChainedError>(thread_name: &str, e: &E) -> Result<String> {
    let mut buf = String::new();
    writeln!(buf, "Thread {} errored with '{}'", thread_name, e)?;
    for e in e.iter().skip(1) {
        writeln!(buf, "Caused by: {}", e)?;
    }
    Ok(buf.trim().to_string())
}
fn cause_from_panic(thread_name: &str, info: &(Any + Send), loc: Option<&Location>) -> String {
    let raw_cause: Cow<'static, str> = if let Some(&s) = info.downcast_ref::<&str>() {
        s.to_string().into()
    } else if let Some(s) = info.downcast_ref::<String>() {
        s.clone().into()
    } else {
        "<could not get panic error>".into()
    };
    let raw_location: Cow<'static, str> = loc.map_or("".into(), |loc| {
        format!(" at {}:{}", loc.file(), loc.line()).into()
    });
    format!("Thread '{}' panicked with '{}'{}", thread_name, raw_cause, raw_location)
}

#[derive(Debug)]
enum PanicInfoStatus {
    DefaultMode, PanicReady, Error(String, Error), PanicInfo(String, Backtrace),
}
thread_local! {
    static PANIC_INFO: RefCell<PanicInfoStatus> = RefCell::new(PanicInfoStatus::DefaultMode);
}

fn thread_name() -> String {
    thread::current().name().or(Some("<unknown>")).unwrap().to_string()
}
pub fn init_panic_hook() {
    let default_hook = take_hook();
    set_hook(Box::new(move |panic_info| {
        PANIC_INFO.with(|info| {
            let mut info = info.borrow_mut();
            match &*info {
                &PanicInfoStatus::Error(_, _) | &PanicInfoStatus::PanicInfo(_, _) => { }
                &PanicInfoStatus::PanicReady => {
                    let cause = cause_from_panic(&thread_name(),
                                                 panic_info.payload(), panic_info.location());
                    *info = PanicInfoStatus::PanicInfo(cause, Backtrace::new());
                }
                &PanicInfoStatus::DefaultMode => default_hook(panic_info),
            }
        });
    }))
}

fn write_error_report<P: AsRef<Path>>(root_path: P, kind: &str, report: &str) -> Result<PathBuf> {
    let mut path = PathBuf::from(root_path.as_ref());
    path.push("logs");
    fs::create_dir_all(&path)?;
    let file_name = format!("{}_report_{}.log",
                            kind.to_lowercase(), Utc::now().format("%Y%m%d_%H%M%S%f"));
    path.push(file_name);
    let mut out = File::create(&path)?;
    out.write_all(report.as_bytes())?;
    Ok(path)
}

fn internal_fatal_error(err: &str) -> ! {
    eprintln!("{}", err);
    abort()
}

struct PanicInfo {
    kind: &'static str, cause: String, backtrace: String,
}
impl PanicInfo {
    fn from_info(info: PanicInfoStatus) -> Result<PanicInfo> {
        match info {
            PanicInfoStatus::Error(thread, err) => {
                let cause = cause_from_error(&thread, &err)?;
                let backtrace = make_backtrace_str(err.backtrace());
                Ok(PanicInfo { kind: "Error", cause, backtrace })
            },
            PanicInfoStatus::PanicInfo(cause, backtrace) => {
                let backtrace = make_backtrace_str(Some(&backtrace));
                Ok(PanicInfo { kind: "Panic", cause, backtrace })
            }
            _ => internal_fatal_error("PanicInfo::from_info called on non-error status."),
        }
    }

    fn make_report(&self) -> Result<String> {
        make_error_report(&self.cause, &self.backtrace, self.kind)
    }
    fn write_report<P: AsRef<Path>>(&self, root_path: P) -> Result<PathBuf> {
        write_error_report(root_path, &self.kind.to_lowercase(), &self.make_report()?)
    }
    fn write_report_with_message<P: AsRef<Path>>(&self, root_path: P) -> Result<()> {
        let lc_kind = self.kind.to_lowercase();
        let report_file = self.write_report(root_path)?;
        for line in self.cause.split("\n") {
            error!("{}", line);
        }
        error!("Detailed information about this {} can be found at '{}'.",
               lc_kind, report_file.display());
        Ok(())
    }
}

pub fn unwrap_fatal<T>(res: Result<T>) -> T {
    match res {
        Ok(t) => t,
        Err(e) => PANIC_INFO.with(|info| {
            if let &PanicInfoStatus::PanicReady = &*info.borrow() {
                *info.borrow_mut() = PanicInfoStatus::Error(thread_name(), e);
                resume_unwind(box "unwrap_fatal() called on Err")
            } else {
                panic!("{}", ChainedError::display_chain(&e))
            }
        })
    }
}
pub fn catch_panic<P: AsRef<Path>, F, R>(root_path: P, f: F) -> Result<R>
    where F: FnOnce() -> R + UnwindSafe {

    PANIC_INFO.with(|info| {
        let old_info = replace(&mut *info.borrow_mut(), PanicInfoStatus::PanicReady);
        let result = catch_unwind(f);
        let current_info = replace(&mut *info.borrow_mut(), old_info);
        match result {
            Ok(r) => if let PanicInfoStatus::PanicReady = current_info {
                Ok(r)
            } else {
                internal_fatal_error("PANIC_INFO != PanicInfoStatus::PanicReady, but got Some!")
            },
            Err(_) => {
                PanicInfo::from_info(current_info)?.write_report_with_message(root_path)?;
                bail!(ErrorKind::Panicked)
            }
        }
    })
}
pub fn report_error<P: AsRef<Path>, T>(root_path: P, res: Result<T>) -> Result<T> {
    match res {
        Ok(t) => Ok(t),
        Err(e) => {
            let info = PanicInfo::from_info(PanicInfoStatus::Error(thread_name(), e))?;
            info.write_report_with_message(root_path)?;
            bail!(ErrorKind::Panicked) // TODO: Report the actual error?
        }
    }
}