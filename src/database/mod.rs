use errors::*;
use r2d2::{Pool, ManageConnection, PooledConnection};
use rusqlite::{Connection, OpenFlags, Rows, TransactionBehavior,
               Row as RusqliteRow, Result as RusqliteResult, Error as RusqliteError};
use rusqlite::types::{ToSql as RusqliteToSql, FromSql as RusqliteFromSql,
                      FromSqlResult, FromSqlError};
use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::thread::panicking;
use std::time::Duration;
use std::sync::Arc;

mod impls;

pub use rusqlite::types::{ToSqlOutput, Value, ValueRef};

pub trait FromSql: Sized {
    fn from_sql(value: ValueRef) -> Result<Self>;
}
struct FromSqlWrapper<T>(T);
impl <T : FromSql> RusqliteFromSql for FromSqlWrapper<T> {
    fn column_result(value: ValueRef) -> FromSqlResult<Self> {
        T::from_sql(value)
            .map(FromSqlWrapper)
            .map_err(|x| FromSqlError::Other(box x.to_sync_error()))
    }
}
pub trait FromSqlRow : Sized {
    fn from_sql_row(row: Row) -> Result<Self>;
}

pub trait ToSql {
    fn to_sql(&self) -> Result<ToSqlOutput>;
}
struct ToSqlWrapper<'a, T: 'a>(&'a T);
impl <'a, T : ToSql + 'a> RusqliteToSql for ToSqlWrapper<'a, T> {
    fn to_sql(&self) -> RusqliteResult<ToSqlOutput> {
        self.0.to_sql().map_err(|x| RusqliteError::ToSqlConversionFailure(box x.to_sync_error()))
    }
}
pub trait ToSqlArgs {
    fn to_sql_args<R, F>(&self, f: F) -> Result<R> where F: FnOnce(&[&RusqliteToSql]) -> Result<R>;
}

struct RowsWrapper<'a>(Rows<'a>);
impl <'a> RowsWrapper<'a> {
    pub fn get_all<T: FromSqlRow>(&mut self) -> Result<Vec<T>> {
        let mut vec = Vec::new();
        while let Some(r) = self.0.next() {
            vec.push(T::from_sql_row(Row(r?))?);
        }
        Ok(vec)
    }

    pub fn get_opt<T: FromSqlRow>(&mut self) -> Result<Option<T>> {
        match self.0.next() {
            Some(r) => Ok(Some(T::from_sql_row(Row(r?))?)),
            None => Ok(None),
        }
    }
    pub fn get<T: FromSqlRow>(&mut self) -> Result<T> {
        Ok(self.get_opt()?.chain_err(|| "Query returned no rows!")?)
    }
}

pub struct QueryDSL<'a, 'b, T> {
    conn: &'a DatabaseConnection, sql: &'b str, cache: bool, args: T,
}
impl <'a, 'b, T: ToSqlArgs> QueryDSL<'a, 'b, T> {
    fn do_op<F, R>(&self, f: F) -> Result<R> where F: FnOnce(RowsWrapper) -> Result<R> {
        self.args.to_sql_args(|args| if self.cache {
            let mut stat = self.conn.conn.prepare(self.sql)?;
            let row = RowsWrapper(stat.query(args)?);
            f(row)
        } else {
            let mut stat = self.conn.conn.prepare_cached(self.sql)?;
            let row = RowsWrapper(stat.query(args)?);
            f(row)
        })
    }

    pub fn get_all<R: FromSqlRow>(&self) -> Result<Vec<R>> {
        self.do_op(|mut x| x.get_all())
    }
    pub fn get_opt<R: FromSqlRow>(&self) -> Result<Option<R>> {
        self.do_op(|mut x| x.get_opt())
    }
    pub fn get<R: FromSqlRow>(&self) -> Result<R> {
        self.do_op(|mut x| x.get())
    }
}

pub struct Row<'a, 'b>(RusqliteRow<'a, 'b>);
impl <'a, 'b> Row<'a, 'b> {
    pub fn len(&self) -> usize {
        self.0.column_count() as usize
    }

    pub fn get<T : FromSql>(&self, i: usize) -> Result<T> {
        ensure!(i <= i32::max_value() as usize, "index out of range!");
        Ok(self.0.get_checked::<i32, FromSqlWrapper<T>>(i as i32)?.0)
    }
}

struct SqliteConnection {
    conn: Connection, is_poisoned: Cell<bool>,
}
impl Deref for SqliteConnection {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}
impl DerefMut for SqliteConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

struct ConnectionManager {
    db_file: PathBuf,
}
impl ConnectionManager {
    fn new(path: &Path) -> Result<ConnectionManager> {
        Ok(ConnectionManager {
            db_file: path.to_owned(),
        })
    }
}
impl ManageConnection for ConnectionManager {
    type Connection = SqliteConnection;
    type Error = Error;

    fn connect(&self) -> Result<SqliteConnection> {
        let conn = Connection::open_with_flags(&self.db_file,
            OpenFlags::SQLITE_OPEN_READ_WRITE |
            OpenFlags::SQLITE_OPEN_CREATE |
            OpenFlags::SQLITE_OPEN_SHARED_CACHE)?;
        conn.set_prepared_statement_cache_capacity(64);
        conn.execute_batch(include_str!("setup_connection.sql"))?;
        Ok(SqliteConnection { conn, is_poisoned: Cell::new(false) })
    }
    fn is_valid(&self, conn: &mut SqliteConnection) -> Result<()> {
        ensure!(!conn.is_poisoned.get(), "Connection poisoned.");
        conn.prepare_cached("SELECT 1")?.query_row(&[], |_| ())?;
        Ok(())
    }
    fn has_broken(&self, conn: &mut SqliteConnection) -> bool {
        !conn.is_poisoned.get()
    }
}

struct TransactionDepthGuard<'a>(usize, &'a Cell<usize>, &'a Cell<bool>);
impl <'a> Drop for TransactionDepthGuard<'a> {
    fn drop(&mut self) {
        if panicking() {
            self.2.set(true)
        } else {
            self.1.set(self.0)
        }
    }
}
impl <'a> TransactionDepthGuard<'a> {
    fn increment(conn: &DatabaseConnection) -> TransactionDepthGuard {
        let get = conn.transaction_depth.get();
        conn.transaction_depth.set(get + 1);
        TransactionDepthGuard(get, &conn.transaction_depth, &conn.conn.is_poisoned)
    }
}

// TODO: Track active connection count for shutdown?
pub struct DatabaseConnection {
    conn: PooledConnection<ConnectionManager>, transaction_depth: Cell<usize>,
}
impl DatabaseConnection {
    fn new(conn: PooledConnection<ConnectionManager>) -> DatabaseConnection {
        DatabaseConnection { conn, transaction_depth: Cell::new(0) }
    }

    // TODO: Track the current type of the Transaction, IMMEDIATE in IMMEDIATE is OK, for example.
    fn transaction_raw<T, F>(
        &self, f: F, behavior: TransactionBehavior
    ) -> Result<T> where F: FnOnce() -> Result<T> {
        let cur_depth = self.transaction_depth.get();
        if cur_depth == 0 {
            let sql = match behavior {
                TransactionBehavior::Deferred  => "BEGIN DEFERRED",
                TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
                TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
            };
            self.execute_cached(sql, ()).unwrap();
            let _depth = TransactionDepthGuard::increment(self);
            match f() {
                Ok(value) => {
                    self.execute_cached("COMMIT", ()).unwrap();
                    Ok(value)
                }
                Err(e) => {
                    self.execute_cached("ROLLBACK", ()).unwrap();
                    Err(e)
                }
            }
        } else if let TransactionBehavior::Deferred = behavior {
            let name = format!("sylph_save_{}", self.transaction_depth.get());
            self.execute(&format!("SAVEPOINT {}", name), ()).unwrap();
            let _depth = TransactionDepthGuard::increment(self);
            match f() {
                Ok(value) => {
                    self.execute(&format!("RELEASE {}", name), ()).unwrap();
                    Ok(value)
                }
                Err(e) => {
                    self.execute(&format!("ROLLBACK TO {}", name), ()).unwrap();
                    Err(e)
                }
            }
        } else {
            bail!("Nested transactions can only be created using DEFERRED.")
        }
    }

    pub fn transaction<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transaction_raw(f, TransactionBehavior::Deferred)
    }
    pub fn transaction_immediate<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transaction_raw(f, TransactionBehavior::Immediate)
    }
    pub fn transaction_exclusive<T, F>(&self, f: F) -> Result<T> where F: FnOnce() -> Result<T> {
        self.transaction_raw(f, TransactionBehavior::Exclusive)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    pub fn execute<T : ToSqlArgs>(&self, sql: &str, args: T) -> Result<isize> {
        Ok(args.to_sql_args(|args| Ok(self.conn.execute(sql, args)?))? as isize)
    }
    pub fn query<'a, 'b, T : ToSqlArgs>(
        &'a self, sql: &'b str, args: T
    ) -> QueryDSL<'a, 'b, T> {
        QueryDSL { conn: self, sql, cache: false, args }
    }

    pub fn execute_cached<T : ToSqlArgs>(&self, sql: &str, args: T) -> Result<isize> {
        Ok(args.to_sql_args(|args| Ok(self.conn.prepare_cached(sql)?.execute(args)?))? as isize)
    }
    pub fn query_cached<'a, 'b, T : ToSqlArgs>(
        &'a self, sql: &'b str, args: T
    ) -> QueryDSL<'a, 'b, T> {
        QueryDSL { conn: self, sql, cache: true, args }
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.conn.query_row("PRAGMA wal_checkpoint(RESTART)", &[], |_| ())?;
        Ok(())
    }
}

struct Migration {
    id: u32, name: &'static str, source: &'static str
}
macro_rules! migration {
    ($id:expr, $file:expr) => {
        Migration { id: $id, name: $file, source: include_str!($file) }
    }
}
static MIGRATIONS: &'static [Migration] = &[
    migration!(1, "version_1.sql"),
];

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

        let created_count = conn.execute(
            "CREATE TABLE IF NOT EXISTS sylph_verifier_migrations (id INTEGER PRIMARY KEY);", ()
        )?;
        if created_count != 0 {
            debug!("Created migrations tracking table.");
        }

        let mut migrations_to_run = Vec::new();
        for migration in MIGRATIONS {
            let count = conn.query_cached("SELECT COUNT(*) FROM sylph_verifier_migrations \
                                           WHERE id=?1", migration.id).get::<u32>()?;
            if count != 1 {
                migrations_to_run.push(migration);
            }
        }

        if !migrations_to_run.is_empty() {
            // TODO: Backup old database
            for migration in migrations_to_run {
                debug!("Running migration '{}'", migration.name);
                conn.transaction_exclusive(|| {
                    conn.execute("INSERT INTO sylph_verifier_migrations (id) VALUES (?1)",
                                 migration.id)?;
                    conn.execute_batch(migration.source)?;
                    Ok(())
                })?;
            }
        }

        conn.checkpoint()?;
        Ok(())
    }
}