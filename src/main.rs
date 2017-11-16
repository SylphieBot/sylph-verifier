// TODO: Remove this when global allocators are stabilized.
#![feature(global_allocator, allocator_api, alloc_system)]
extern crate alloc_system;
#[global_allocator] static ALLOC: alloc_system::System = alloc_system::System;

extern crate byteorder;
extern crate constant_time_eq;
extern crate hmac;
extern crate rusqlite;
extern crate sha2;
extern crate uuid;

#[macro_use] extern crate error_chain;
#[macro_use] extern crate serenity;

mod errors;
mod lz4;
mod place;
mod token;

fn main() {
    let ctx = token::TokenContext::new("test_key".as_bytes(), 300);
    println!("{}", ctx.make_token(436430689, 0).unwrap());

    use std::io::Write;
    let place_data = place::create_place_file(None, vec![
        place::LuaConfigEntry::new("test_a", true , "test"),
        place::LuaConfigEntry::new("test_b", false, "test"),
    ]).unwrap();
    ::std::fs::File::create("test.rbxl").unwrap().write(&place_data).unwrap();
}
