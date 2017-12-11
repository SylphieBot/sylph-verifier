use core::*;
use dotenv;
use errors::*;
use logger;
use std::env;
use std::path::PathBuf;

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
        Some(PathBuf::from(env::var_os("DATABASE_URL")
            .expect("cannot get DATABASE_URL environment variable")))
    } else {
        None
    };

    // Setup logging
    logger::init(&root_path).expect("failed to setup logging");

    // Init core
    let core = VerifierCore::new(root_path, db_path).expect("failed to initialize core");

    // Start bot
    core.start().unwrap();
}