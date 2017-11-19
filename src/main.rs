extern crate byteorder;
extern crate constant_time_eq;
extern crate dotenv;
extern crate fs2;
extern crate hmac;
extern crate libc;
extern crate parking_lot;
extern crate rusqlite;
extern crate sha2;
extern crate thread_local;
extern crate uuid;

#[macro_use] extern crate diesel;
#[macro_use] extern crate diesel_codegen;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate log;
#[macro_use] extern crate serenity;

mod boot;
mod database;
mod errors;
mod roblox;
mod token;
mod util;

// TODO: Ensure panics/etc stay onscreen long enough to read.

fn main() {
    dotenv::dotenv().expect("dotenv error");

    let ret = boot::start();
    if ret != 0 {
        ::std::process::exit(ret)
    }
}
