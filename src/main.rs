#![warn(unused_extern_crates)]
#![recursion_limit="128"]

extern crate byteorder;
extern crate constant_time_eq;
extern crate dotenv;
extern crate fs2;
extern crate hmac;
extern crate hyper;
extern crate parking_lot;
extern crate percent_encoding;
extern crate r2d2;
extern crate rand;
extern crate regex;
extern crate select;
extern crate sha2;
extern crate uuid;

#[macro_use] extern crate diesel;
#[macro_use] extern crate diesel_codegen;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate lazy_static;
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
    dotenv::dotenv().ok();

    let ret = boot::start();
    if ret != 0 {
        ::std::process::exit(ret)
    }
}
