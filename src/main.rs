#![warn(unused_extern_crates)]
#![recursion_limit="128"]
#![feature(box_syntax, box_patterns, never_type, integer_atomics, const_atomic_u8_new)]

extern crate byteorder;
extern crate chrono;
extern crate constant_time_eq;
extern crate dotenv;
extern crate fs2;
extern crate hmac;
extern crate linefeed;
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
mod core;
mod discord;
mod errors;
mod roblox;

fn main() {
    match boot::start() {
        Some(ret) => ::std::process::exit(ret),
        None => loop {
            ::std::thread::sleep(::std::time::Duration::from_secs(60))
        },
    }
}
