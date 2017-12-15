use core::*;
use error_report;
use errors::*;
use fs2::*;
use logger;
use std::env;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::abort;

const LOCK_FILE_NAME: &'static str = "Sylph-Verifier.lock";
const DB_FILE_NAME: &'static str = "Sylph-Verifier.db";

fn check_lock<P: AsRef<Path>>(path: P) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    let lock_file = options.open(path)?;
    lock_file.try_lock_exclusive()?;
    Ok(lock_file)
}
fn in_path<P: AsRef<Path>>(root_path: P, file: &str) -> PathBuf {
    let mut path = PathBuf::new();
    path.push(root_path);
    path.push(file);
    path
}

fn get_exe_dir() -> PathBuf {
    let mut path = env::current_exe().expect("cannot get current exe path");
    path.pop();
    path
}
macro_rules! check_env {
    ($e:expr) => { env::var($e).ok().map_or(false, |x| x == env!($e)) }
}
fn is_cargo_launch() -> bool {
    check_env!("CARGO_PKG_NAME") && check_env!("CARGO_PKG_VERSION") && env::var("CARGO").is_ok()
}
fn get_root_path() -> PathBuf {
    if is_cargo_launch() {
        match env::var_os("CARGO_MANIFEST_DIR") {
            Some(manifest_dir) => {
                let buf = PathBuf::from(manifest_dir);
                let mut toml_file = buf.clone();
                toml_file.push("Cargo.toml");
                if toml_file.exists() {
                    buf
                } else {
                    get_exe_dir()
                }
            }
            None => get_exe_dir()
        }
    } else {
        get_exe_dir()
    }
}

pub fn start() {
    // Get RUST_BACKTRACE=1 cached by error_chain.
    let original = env::var_os("RUST_BACKTRACE");
    env::set_var("RUST_BACKTRACE", "1");
    Error::from_kind(ErrorKind::Panicked);
    match original {
        Some(x) => env::set_var("RUST_BACKTRACE", x),
        None => env::remove_var("RUST_BACKTRACE"),
    }

    // Find paths
    let root_path = get_root_path();
    let db_path = in_path(&root_path, DB_FILE_NAME);

    // Acquire the lock file.
    let _lock = match check_lock(in_path(&root_path, LOCK_FILE_NAME)) {
        Ok(lock) => lock,
        Err(_) => {
            println!("Only one instance of Sylph-Verifier may be launched at once.");
            abort()
        }
    };

    // Setup logging
    logger::init(&root_path).expect("failed to setup logging");
    error_report::init(&root_path);

    // Start bot proper
    error_report::catch_error(move || {
        debug!("Root directory: {}", root_path.display());
        debug!("Database path: {}", db_path.display());

        VerifierCore::new(db_path)?.start()?;
        Ok(())
    }).ok();
}