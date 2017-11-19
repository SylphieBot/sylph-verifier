use std::env::current_exe;
use std::path::*;

pub fn bin_relative(buf: &str) -> PathBuf {
    let mut path = current_exe().expect("cannot get current exe path");
    path.pop();
    path.push(buf);
    path
}
