use core::*;
use database::Database;
use error_report;
use errors::*;
use fs2::*;
use logger;
use std::env;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::abort;

const LOCK_FILE_NAME: &str = "Sylph-Verifier.lock";
const DB_FILE_NAME: &str = "Sylph-Verifier.db";

fn check_lock(path: impl AsRef<Path>) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    let lock_file = options.open(path)?;
    lock_file.try_lock_exclusive()?;
    Ok(lock_file)
}
fn in_path(root_path: impl AsRef<Path>, file: &str) -> PathBuf {
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

#[cfg(feature = "release")]
const IS_RELEASE: bool = true;
#[cfg(not(feature = "release"))]
const IS_RELEASE: bool = false;

fn get_root_path() -> PathBuf {
    if IS_RELEASE {
        get_exe_dir()
    } else {
        match env::var_os("CARGO_MANIFEST_DIR") {
            Some(manifest_dir) => PathBuf::from(manifest_dir),
            None => get_exe_dir(),
        }
    }
}

pub fn start() {
    env::set_var("RUST_FAILURE_BACKTRACE", "1");

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

        let database = Database::new(db_path)?;
        VerifierCore::new(root_path, database)?.start()?;
        Ok(())
    }).ok();
}