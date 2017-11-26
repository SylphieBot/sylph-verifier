use core::*;
use dotenv;
use errors::*;
use roblox::*;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

fn verification_test(ids: &[RobloxUserID]) -> Result<()> {
    let rules = VerificationSet::compile(
        &["FormerBC", "NotBC", "BC", "TBC", "OBC", "DevForum", "RobloxAdmin",
          "FormerAccelerator", "FormerIncubator", "FormerIntern", "Accelerator",
          "Incubator", "Intern"],
        ::std::collections::HashMap::new())?;
    info!("{:#?}", rules);
    for &id in ids {
        let id_name = id.lookup_username()?;
        rules.verify(id, |name, bool| {
            info!("{} {} {}", &id_name, name, bool);
            Ok(())
        })?;
    }
    Ok(())
}
pub fn start() -> Option<i32> {
    // Setup .env for development builds.
    dotenv::dotenv().ok();

    // Set up custom error handling
    init_panic_hook();

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

    // Init core
    let core = Arc::new(VerifierCore::new(root_path, db_path).expect("failed to initialize core"));

    // TODO: Debug
    info!("Log before terminal.");
    let joiner = thread::Builder::new().name("debug thread".to_owned()).spawn({
        let core = core.clone();
        move || {
            let user = RobloxUserID::for_username("Tiffblocks").unwrap();
            info!("Tiffblocks AAAAAA verify: {:?}", core.check_token(user, "AAAAAA"));

            let users = &[
                RobloxUserID::for_username("Tiffblocks").unwrap(),
                RobloxUserID::for_username("Lymeefairy").unwrap(),
                RobloxUserID::for_username("Lunya").unwrap(),
                RobloxUserID::for_username("fractality").unwrap(),
                RobloxUserID::for_username("NowDoTheHarlemShake").unwrap(),
            ];
            verification_test(users).unwrap();

            use std::io::Write;
            let place_data = create_place_file(None, core.place_config()).unwrap();
            ::std::fs::File::create("test.rbxl").unwrap().write(&place_data).unwrap();
        }
    }).unwrap();

    // Setup terminal
    core.run_terminal().unwrap()
}