use diesel::connection::{Connection, SimpleConnection, TransactionManager};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use errors::*;
use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};
use r2d2::{Pool, ManageConnection, PooledConnection};
use std::borrow::{Cow, ToOwned};
use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::path::*;
use std::time::Duration;
use std::sync::Arc;

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
        let conn = SqliteConnection::establish(&self.db_uri)?;
        conn.batch_execute(include_str!("setup_connection.sql"))?;
        Ok(conn)
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

// TODO: Remove this when something like this ends up in Diesel itself.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum TransactionType {
    Deferred, Immediate, Exclusive,
}

struct SqliteTransactionManager {
    transaction_depth: Cell<u32>,
}
impl SqliteTransactionManager {
    pub fn new() -> Self {
        SqliteTransactionManager { transaction_depth: Cell::new(0) }
    }

    fn begin_transaction(&self, conn: &SqliteConnection,
                         locking: TransactionType) -> Result<()> {
        let transaction_depth = self.transaction_depth.get();
        if transaction_depth == 0 {
            let locking = match locking {
                TransactionType::Deferred  => "DEFERRED",
                TransactionType::Immediate => "IMMEDIATE",
                TransactionType::Exclusive => "EXCLUSIVE",
            };
            conn.execute(&format!("BEGIN {}", locking))?;
        } else {
            ensure!(locking == TransactionType::Deferred,
                    "Already in transaction, cannot create {:?} transaction.", locking);
            conn.execute(&format!("SAVEPOINT sylph_savepoint_{}", transaction_depth))?;
        }
        self.transaction_depth.set(transaction_depth + 1);
        Ok(())
    }
    fn rollback_transaction(&self, conn: &SqliteConnection) -> Result<()> {
        let transaction_depth = self.transaction_depth.get();
        if transaction_depth == 1 {
            conn.execute("ROLLBACK")?;
        } else {
            conn.execute(&format!(
                "ROLLBACK TO SAVEPOINT sylph_savepoint_{}", transaction_depth - 1))?;
        }
        self.transaction_depth.set(transaction_depth - 1);
        Ok(())
    }
    fn commit_transaction(&self, conn: &SqliteConnection) -> Result<()> {
        let transaction_depth = self.transaction_depth.get();
        if transaction_depth <= 1 {
            conn.execute("COMMIT")?;
        } else {
            conn.execute(&format!(
                "RELEASE SAVEPOINT sylph_savepoint_{}", transaction_depth - 1))?;
        }
        self.transaction_depth.set(transaction_depth - 1);
        Ok(())
    }
}

pub struct DatabaseConnection {
    conn: PooledConnection<ConnectionManager>,
    transactions: SqliteTransactionManager,
}
impl DatabaseConnection {
    fn new(conn: PooledConnection<ConnectionManager>) -> DatabaseConnection {
        DatabaseConnection { conn, transactions: SqliteTransactionManager::new() }
    }

    // So we don't have to import std::ops in everything
    pub fn deref(&self) -> &SqliteConnection {
        self.conn.deref()
    }
    pub fn deref_mut(&mut self) -> &mut SqliteConnection {
        self.conn.deref_mut()
    }

    fn transaction_raw<T, F>(
        &self, f: F, kind: TransactionType
    ) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transactions.begin_transaction(self.deref(), kind)?;
        match f() {
            Ok(value) => {
                self.transactions.commit_transaction(self.deref())?;
                Ok(value)
            }
            Err(e) => {
                self.transactions.rollback_transaction(self.deref())?;
                Err(e)
            }
        }
    }

    pub fn transaction<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transaction_raw(f, TransactionType::Deferred)
    }
    pub fn transaction_immediate<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transaction_raw(f, TransactionType::Immediate)
    }
    pub fn transaction_exclusive<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transaction_raw(f, TransactionType::Exclusive)
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.deref().execute("PRAGMA wal_checkpoint(RESTART);")?;
        Ok(())
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

#[derive(Clone)]
pub struct Database {
    pool: Arc<Pool<ConnectionManager>>,
}
impl Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Database> {
        let pool = Arc::new(Pool::builder()
            .max_size(15)
            .idle_timeout(Some(Duration::from_secs(60 * 5)))
            .build(ConnectionManager::new(path.as_ref())?)?);

        let database = Database { pool };
        database.init_db()?;
        Ok(database)
    }

    pub fn connect(&self) -> Result<DatabaseConnection> {
        Ok(DatabaseConnection::new(self.pool.get()?))
    }

    fn init_db(&self) -> Result<()> {
        let conn = self.connect()?;
        embedded_migrations::run(conn.deref())?;
        conn.checkpoint()?;
        Ok(())
    }
}

pub trait CustomDBType {
    type Underlying_To: ?Sized + ToOwned;
    fn to_underlying(&self) -> Cow<Self::Underlying_To>;

    type Underlying_From;
    fn from_underlying(underlying: Self::Underlying_From) -> Self;
}
macro_rules! custom_db_type {
    ($t:ident, $mod_name:ident, $db_ty:ident) => {
        mod $mod_name {
            use super::$t;

            use $crate::core::database::CustomDBType;

            use diesel::backend::Backend;
            use diesel::types::*;
            use std::error::Error;
            use std::io::Write;


            impl <DB> ToSql<$db_ty, DB> for $t
                where <$t as CustomDBType>::Underlying_To:ToSql<$db_ty, DB>, DB: Backend {

                fn to_sql<W: Write>(
                    &self, out: &mut ToSqlOutput<W, DB>
                ) -> Result<IsNull, Box<Error + Send + Sync>> {
                    self.to_underlying().to_sql(out)
                }
            }
            impl <DB> FromSql<$db_ty, DB> for $t
                where <$t as CustomDBType>::Underlying_From: FromSql<$db_ty, DB>,
                      DB: Backend + HasSqlType<$db_ty> {

                fn from_sql(
                    bytes: Option<&DB::RawValue>
                ) -> Result<Self, Box<Error + Send + Sync>> {
                    let ul = <$t as CustomDBType>::Underlying_From::from_sql(bytes)?;
                    Ok(Self::from_underlying(ul))
                }
            }

            expression_impls!($db_ty -> $t);
            queryable_impls!($db_ty -> $t);
        }
    }
}