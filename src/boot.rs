use errors::*;
use fs2::*;
use roblox::*;
use std::fs::{File,OpenOptions};
use token::*;
use util::*;

const LOCK_FILE_NAME: &'static str = "Sylph-Verifier.lock";
fn check_lock() -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    let lock_file = options.open(bin_relative(LOCK_FILE_NAME))?;
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

    let ctx = TokenContext::new("test_key".as_bytes(), 300);
    println!("{}", ctx.make_token(436430689, 0).unwrap());

    error_report_test();

    use std::io::Write;
    let place_data = create_place_file(None, vec![
        LuaConfigEntry::new("test_a", true , "test"),
        LuaConfigEntry::new("test_b", false, "test"),
    ]).unwrap();
    ::std::fs::File::create("test.rbxl").unwrap().write(&place_data).unwrap();

    0
}