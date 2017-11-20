#![warn(unused_extern_crates)]
#![recursion_limit="128"]
#![feature(panic_col, box_syntax, box_patterns)]

extern crate byteorder;
extern crate constant_time_eq;
extern crate dotenv;
extern crate fs2;
extern crate hmac;
extern crate parking_lot;
extern crate percent_encoding;
extern crate r2d2;
extern crate rand;
extern crate regex;
extern crate reqwest;
extern crate serde_json;
extern crate sha2;
extern crate uuid;

#[allow(unused_extern_crates)] extern crate serde;

#[macro_use] extern crate diesel;
#[macro_use] extern crate diesel_codegen;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
#[macro_use] extern crate nom;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serenity;

mod boot;
mod database;
mod discord;
mod errors;
mod roblox;
mod token;
mod util;

// TODO: Ensure panics/etc stay onscreen long enough to read.

fn main() {
    // Setup .env for development builds.
    dotenv::dotenv().ok();

    // Set up custom error handling
    errors::init_panic_hook();

    // Get RUST_BACKTRACE=1 cached by error_chain.
    let original = ::std::env::var_os("RUST_BACKTRACE");
    ::std::env::set_var("RUST_BACKTRACE", "1");
    errors::Error::from_kind(errors::ErrorKind::Panicked);
    match original {
        Some(x) => ::std::env::set_var("RUST_BACKTRACE", x),
        None => ::std::env::remove_var("RUST_BACKTRACE"),
    }

    let ret = boot::start();
    if ret != 0 {
        ::std::process::exit(ret)
    }
}
