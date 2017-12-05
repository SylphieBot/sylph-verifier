use diesel::connection::Connection;
use diesel::sqlite::SqliteConnection;
use errors::*;
use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};
use r2d2::{Pool, ManageConnection, PooledConnection};
use std::ops::{Deref, DerefMut};
use std::path::*;
use std::time::Duration;

struct ConnectionManager {
    db_uri: String,
}
impl ConnectionManager {
    pub fn new(path: &Path) -> Result<ConnectionManager> {
        let path = path.to_str().chain_err(|| format!("invalid db path: {}", path.display()))?;
        Ok(ConnectionManager {
            db_uri: format!("file:{}?mode=rwc&cache=shared",
                            utf8_percent_encode(path, DEFAULT_ENCODE_SET))
        })
    }
}
impl ManageConnection for ConnectionManager {
    type Connection = SqliteConnection;
    type Error = Error;

    fn connect(&self) -> Result<SqliteConnection> {
        Ok(SqliteConnection::establish(&self.db_uri)?)
    }
    fn is_valid(&self, _conn: &mut SqliteConnection) -> Result<()> {
        // TODO: I don't think sqlite connections can break
        Ok(())
    }
    fn has_broken(&self, _conn: &mut SqliteConnection) -> bool {
        false
    }
}

pub mod schema {
    infer_schema!("dotenv:DATABASE_URL");
}
embed_migrations!();

pub struct DatabaseConnection(PooledConnection<ConnectionManager>);
impl DatabaseConnection {
    // So we don't have to import std::ops in everything
    pub fn deref(&self) -> &SqliteConnection {
        self.0.deref()
    }
    pub fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.0.deref_mut()
    }

    pub fn transaction<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.0.transaction(f)
    }
}
impl Deref for DatabaseConnection {
    type Target = SqliteConnection;
    fn deref(&self) -> &SqliteConnection {
        self.deref()
    }
}
impl DerefMut for DatabaseConnection {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.deref_mut()
    }
}

pub struct Database {
    pool: Pool<ConnectionManager>,
}
impl Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Database> {
        let path = path.as_ref();
        let pool = Pool::builder()
            .max_size(15)
            .idle_timeout(Some(Duration::from_secs(60 * 5)))
            .build(ConnectionManager::new(path)?)?;

        let database = Database { pool };
        database.init_db()?;
        Ok(database)
    }

    pub fn connect(&self) -> Result<DatabaseConnection> {
        Ok(DatabaseConnection(self.pool.get()?))
    }

    fn init_db(&self) -> Result<()> {
        let conn = self.connect()?;
        embedded_migrations::run(conn.deref())?;
        Ok(())
    }
}