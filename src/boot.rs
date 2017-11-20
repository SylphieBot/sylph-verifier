use database::*;
use errors::*;
use fs2::*;
use roblox::*;
use std::fs::{File,OpenOptions};
use token::*;
use util::*;

const LOCK_FILE_NAME: &'static str = "Sylph-Verifier.lock";
const DB_FILE_NAME: &'static str = "Sylph-Verifier.db";

fn check_lock() -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    let lock_file = options.open(bin_relative(LOCK_FILE_NAME)?)?;
    lock_file.try_lock_exclusive()?;
    Ok(lock_file)
}
pub fn start() -> i32 {
    let _lock_file = match check_lock() {
        Ok(lock) => lock,
        Err(_) => {
            error!("Only one instance of Sylph-Verifier may be launched at once.");
            return 1
        }
    };
    let db = Database::new(bin_relative(DB_FILE_NAME).unwrap()).unwrap();

    let ctx = TokenContext::from_db(&db.connect().expect("database to connect"), 300).unwrap();

    let user = RobloxUserID::for_username("Tiffblocks").unwrap();
    println!("{:?}", ctx.check_token(user, "AAAAAA"));

    use std::io::Write;
    let mut config = Vec::new();
    ctx.add_config(&mut config);
    let place_data = create_place_file(None, config).unwrap();
    ::std::fs::File::create("test.rbxl").unwrap().write(&place_data).unwrap();

    0
}