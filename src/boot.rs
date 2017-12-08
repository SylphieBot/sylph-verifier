use core::*;
use dotenv;
use errors::*;
use roblox::*;
use std::env;
use std::path::PathBuf;
use std::thread;

fn verification_test(ids: &[RobloxUserID]) -> Result<()> {
    let rules = VerificationSet::compile(
        &["Verified", "FormerBC", "NotBC", "BC", "TBC", "OBC", "DevForum",
          "RobloxAdmin", "FormerAccelerator", "FormerIncubator", "FormerIntern",
          "Accelerator", "Incubator", "Intern"],
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

    // Init core
    let core = VerifierCore::new(root_path, db_path).expect("failed to initialize core");

    // TODO: Debug
    info!("Log before terminal.");
    let joiner = thread::Builder::new().name("debug thread".to_owned()).spawn({
        let core = core.clone();
        move || {
            core.catch_panic(|| panic!("test")).ok();
            core.catch_error(|| {
                let users = &[
                    RobloxUserID::for_username("Tiffblocks")?,
                    RobloxUserID::for_username("Lymeefairy")?,
                    RobloxUserID::for_username("Lunya")?,
                    RobloxUserID::for_username("fractality")?,
                    RobloxUserID::for_username("NowDoTheHarlemShake")?,
                ];
                verification_test(users)?;

                use std::io::Write;
                let place_data = create_place_file(None, core.place_config())?;
                ::std::fs::File::create("test.rbxl")?.write(&place_data)?;

                Ok(())
            }).ok();
        }
    }).unwrap();

    // Setup terminal
    core.start().unwrap();
}