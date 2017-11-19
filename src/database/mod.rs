use errors::*;
use libc::c_int;
use parking_lot::*;
use rusqlite::{Connection, OpenFlags, Result as SqlResult};
use serenity::model::UserId;
use std::path::*;
use std::sync::Arc;
use std::sync::atomic::*;
use thread_local::*;

fn raw_open_db(file: &Path) -> Result<Connection> {
    const SQLITE_OPEN_READ_WRITE  : c_int = 0x00000002;
    const SQLITE_OPEN_CREATE      : c_int = 0x00000004;
    const SQLITE_OPEN_SHARED_CACHE: c_int = 0x00020000;
    Ok(Connection::open_with_flags(file, OpenFlags::from_bits(
        SQLITE_OPEN_READ_WRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_SHARED_CACHE
    ).unwrap())?)
}

const INIT_DATABASE_QUERY: &'static str = include_str!("init_database.sql");
fn init_db(file: &Path) -> Result<()> {
    let conn = raw_open_db(file)?;
    if conn.query_row_and_then("SELECT count(*) FROM sqlite_master \
                                WHERE type = 'table' AND name = 'sylph_verifier_version'", &[],
                               |row| -> SqlResult<u32> { row.get_checked(0) })? == 0 {
        info!("Initializing Slyph-Verifier database...");
        conn.execute_batch(INIT_DATABASE_QUERY)?;
    } else {
        info!("Checking database version...");
        match conn.query_row_and_then("SELECT version FROM sylph_verifier_version", &[],
                                      |row| -> SqlResult<u32> { row.get_checked(0) })? {
            1 => { }
            _ => bail!(ErrorKind::UnknownDatabaseVersion),
        }
    }
    Ok(())
}

pub struct VerificationData {
    discord_id: UserId,
}

pub struct DatabaseConnection {
    conn: Connection,
}
impl DatabaseConnection {
    pub fn is_user_verified(&self, user_id: UserId) -> Result<Option<VerificationData>> {
        unimplemented!()
    }
}

pub struct Database {
    db_path: PathBuf,
}
impl Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Database> {
        let path = path.as_ref();
        init_db(path)?;
        Ok(Database { db_path: path.to_owned() })
    }


}