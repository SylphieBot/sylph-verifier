#![warn(unused_extern_crates)]
#![recursion_limit="128"]
#![feature(nll, crate_visibility_modifier)]
#![feature(box_patterns, never_type, integer_atomics, const_fn, try_trait)]
#![deny(unused_must_use)]

// TODO: Pass around IDs less to touch Serenity's cache less.
// TODO: Add logging for verifications to a log channel.
// TODO: Rewrite to be async.

extern crate backtrace;
extern crate byteorder;
extern crate chrono;
extern crate constant_time_eq;
extern crate fs2;
extern crate hmac;
extern crate hyper;
extern crate linefeed;
extern crate num_cpus;
extern crate parking_lot;
extern crate percent_encoding;
extern crate r2d2;
extern crate rand;
extern crate regex;
extern crate reqwest;
extern crate rusqlite;
extern crate serde_json;
extern crate serenity;
extern crate sha2;
extern crate threadpool;
extern crate uuid;

#[allow(unused_extern_crates)] extern crate serde;

#[macro_use] extern crate enumset;
#[macro_use] extern crate failure;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
#[macro_use] extern crate serde_derive;
extern crate scraper;

#[macro_use] pub mod errors;

pub mod commands;
pub mod core;
pub mod database;
pub mod error_report;
pub mod logger;
pub mod roblox;
pub mod startup;
pub mod util;

fn main() {
    println!("Sylph-Verifier v{} by LymeeFairy", env!("CARGO_PKG_VERSION"));
    println!("Licenced under the Apache license, version 2.0");
    println!();

    startup::start();
    std::process::exit(0); // Just in case.
}
