use errors::*;
use std::env;
use std::path::PathBuf;

pub fn bin_relative(buf: &str) -> Result<PathBuf> {
    let mut path = env::current_exe().chain_err(|| "cannot get current exe path")?;
    path.pop();
    path.push(buf);
    Ok(path)
}