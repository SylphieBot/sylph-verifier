use core::*;
use dotenv;
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

pub fn start() {
    // Setup .env for development builds.
    dotenv::dotenv().ok();

    // Get RUST_BACKTRACE=1 cached by error_chain.
    let original = env::var_os("RUST_BACKTRACE");
    env::set_var("RUST_BACKTRACE", "1");
    Error::from_kind(ErrorKind::Panicked);
    match original {
        Some(x) => env::set_var("RUST_BACKTRACE", x),
        None => env::remove_var("RUST_BACKTRACE"),
    }

    // Find paths
    let is_dev_mode = match env::var("SYLPH_VERIFIER_DEV_MODE") {
        Ok(s) => s == "true",
        Err(_) => false,
    };
    let root_path = if is_dev_mode {
        env::current_dir().expect("cannot get current directory")
    } else {
        let mut path = env::current_exe().expect("cannot get current exe path");
        path.pop();
        path
    };
    let db_path = if is_dev_mode {
        PathBuf::from(env::var_os("DATABASE_URL")
            .expect("cannot get DATABASE_URL environment variable"))
    } else {
        in_path(&root_path, DB_FILE_NAME)
    };

    // Acquire the lock file.
    let _lock = match check_lock(in_path(&root_path, LOCK_FILE_NAME)) {
        Ok(lock) => lock,
        Err(err) => {
            println!("Only one instance of Sylph-Verifier may be launched at once.");
            abort()
        }
    };

    // Setup logging
    logger::init(&root_path).expect("failed to setup logging");
    error_report::init(&root_path);

    // Start bot proper
    error_report::catch_error(move || {
        let core = VerifierCore::new(db_path)?;
        core.start()?;
        Ok(())
    }).ok();
}