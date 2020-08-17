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
use std::time;
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
            .map_err(|x| FromSqlError::Other(Box::new(x.compat())))
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
        self.0.to_sql().map_err(|x|
            RusqliteError::ToSqlConversionFailure(Box::new(x.compat())))
    }
}
pub trait ToSqlArgs {
    fn to_sql_args<R>(&self, f: impl FnOnce(&[&dyn RusqliteToSql]) -> Result<R>) -> Result<R>;
}

struct RowsWrapper<'a>(Rows<'a>);
impl <'a> RowsWrapper<'a> {
    fn get_all<T: FromSqlRow>(&mut self) -> Result<Vec<T>> {
        let mut vec = Vec::new();
        while let Some(r) = self.0.next()? {
            vec.push(T::from_sql_row(Row(r))?);
        }
        Ok(vec)
    }

    fn get_opt<T: FromSqlRow>(&mut self) -> Result<Option<T>> {
        match self.0.next()? {
            Some(r) => Ok(Some(T::from_sql_row(Row(r))?)),
            None => Ok(None),
        }
    }
    fn get<T: FromSqlRow>(&mut self) -> Result<T> {
        let opt = self.get_opt()?;
        match opt {
            Some(res) => Ok(res),
            None => bail!("Query returned no rows!"),
        }
    }
}

pub struct QueryDSL<'a, 'b, T> {
    conn: &'a DatabaseConnection, sql: &'b str, cache: bool, args: T,
}
impl <'a, 'b, T: ToSqlArgs> QueryDSL<'a, 'b, T> {
    fn do_op<R>(&self, f: impl FnOnce(RowsWrapper) -> Result<R>) -> Result<R> {
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

pub struct Row<'a, 'b>(&'b RusqliteRow<'a>);
impl <'a, 'b> Row<'a, 'b> {
    pub fn len(&self) -> usize {
        self.0.column_count() as usize
    }

    pub fn get<T : FromSql>(&self, i: usize) -> Result<T> {
        Ok(self.0.get::<usize, FromSqlWrapper<T>>(i)?.0)
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
    type Error = RusqliteError;

    fn connect(&self) -> RusqliteResult<SqliteConnection> {
        let conn = Connection::open_with_flags(&self.db_file,
            OpenFlags::SQLITE_OPEN_READ_WRITE |
            OpenFlags::SQLITE_OPEN_CREATE)?;
        conn.set_prepared_statement_cache_capacity(64);
        conn.execute_batch(include_str!("setup_connection.sql"))?;
        Ok(SqliteConnection { conn, is_poisoned: Cell::new(false) })
    }
    fn is_valid(&self, conn: &mut SqliteConnection) -> RusqliteResult<()> {
        if conn.is_poisoned.get() {
            // Random RuqliteError variant.
            return Err(RusqliteError::QueryReturnedNoRows)
        }
        conn.prepare_cached("SELECT 1")?.query(&[] as &[u32])?.next()?;
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
    fn transaction_raw<T>(
        &self, f: impl FnOnce() -> Result<T>, behavior: TransactionBehavior
    ) -> Result<T> {
        let cur_depth = self.transaction_depth.get();
        if cur_depth == 0 {
            let sql = match behavior {
                TransactionBehavior::Deferred  => "BEGIN DEFERRED",
                TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
                TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
                _ => todo!("New transaction behavior added."),
            };
            self.execute(sql, ())?;
            let _depth = TransactionDepthGuard::increment(self);
            match f() {
                Ok(value) => {
                    self.execute("COMMIT", ())?;
                    Ok(value)
                }
                Err(e) => {
                    self.execute("ROLLBACK", ())?;
                    Err(e)
                }
            }
        } else if let TransactionBehavior::Deferred = behavior {
            let name = format!("sylph_save_{}", self.transaction_depth.get());
            self.execute(&format!("SAVEPOINT {}", name), ())?;
            let _depth = TransactionDepthGuard::increment(self);
            match f() {
                Ok(value) => {
                    self.execute(&format!("RELEASE {}", name), ())?;
                    Ok(value)
                }
                Err(e) => {
                    self.execute(&format!("ROLLBACK TO {}", name), ())?;
                    Err(e)
                }
            }
        } else {
            bail!("Nested transactions can only be created using DEFERRED.")
        }
    }

    pub fn transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction_raw(f, TransactionBehavior::Deferred)
    }
    pub fn transaction_immediate<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction_raw(f, TransactionBehavior::Immediate)
    }
    pub fn transaction_exclusive<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction_raw(f, TransactionBehavior::Exclusive)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    pub fn execute(&self, sql: &str, args: impl ToSqlArgs) -> Result<isize> {
        Ok(args.to_sql_args(|args| Ok(self.conn.prepare_cached(sql)?.execute(args)?))? as isize)
    }
    pub fn query<'a, 'b, T : ToSqlArgs>(
        &'a self, sql: &'b str, args: T
    ) -> QueryDSL<'a, 'b, T> {
        QueryDSL { conn: self, sql, cache: true, args }
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(RESTART)")?;
        Ok(())
    }
}

struct Migration {
    from: u32, to: u32, name: &'static str, source: &'static str
}
macro_rules! migration {
    ($from:expr, $to:expr, $file:expr) => {
        Migration { from: $from, to: $to, name: $file, source: include_str!($file) }
    }
}
static MIGRATIONS: &'static [Migration] = &[
    migration!(0, 2, "version_0_to_2.sql"),
    migration!(2, 3, "version_2_to_3.sql"),
];
const CURRENT_VERSION: u32 = 3;
const FUTURE_VERSION_ERR: &str = "This database was created for a future version of this bot. \
                                  Please restore an older version of the database from a backup.";

#[derive(Clone)]
pub struct Database {
    pool: Arc<Pool<ConnectionManager>>,
}
impl Database {
    pub fn new(path: impl AsRef<Path>) -> Result<Database> {
        let pool = Arc::new(Pool::builder()
            .max_size(15)
            .idle_timeout(Some(time::Duration::from_secs(60 * 5)))
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

        conn.transaction_exclusive(|| {
            let meta_table_exists = conn.query(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='sylph_verifier_meta';", (),
            ).get::<u32>()? != 0;
            if !meta_table_exists {
                debug!("Setting up Sylph-Verifier database metadata.");
                conn.execute_batch(
                    "CREATE TABLE sylph_verifier_meta (\
                         key TEXT PRIMARY KEY, value BLOB NOT NULL\
                     ) WITHOUT ROWID;\
                     INSERT INTO sylph_verifier_meta (key, value) VALUES ('meta_version', 1);\
                     INSERT INTO sylph_verifier_meta (key, value) VALUES ('schema_version', 0);"
                )?;
            }

            Ok(())
        })?;

        let meta_version = conn.query(
            "SELECT value FROM sylph_verifier_meta WHERE key = 'meta_version';", ()
        ).get::<u32>()?;
        ensure!(meta_version == 1, FUTURE_VERSION_ERR);

        let mut to_run = Vec::new();
        let mut current_version = conn.query(
            "SELECT value FROM sylph_verifier_meta WHERE key = 'schema_version';", ()
        ).get::<u32>()?;
        ensure!(current_version <= CURRENT_VERSION, FUTURE_VERSION_ERR);

        // TODO: Backup old database.
        for migration in MIGRATIONS {
            if migration.from == current_version {
                to_run.push(migration);
                current_version = migration.to;
            }
        }
        if current_version != CURRENT_VERSION {
            bail!("No migration found from version {} -> {}. Maybe this database was created by \
                   a development version of the bot?", current_version, CURRENT_VERSION);
        }
        for migration in to_run {
            debug!("Running migration '{}'", migration.name);

            // We don't execute this in a transaction to allow the use of PRAGMA foreign_key
            conn.execute_batch(migration.source)?;
            conn.execute(
                "UPDATE sylph_verifier_meta SET value = ?1 WHERE key = \"schema_version\";",
                migration.to,
            )?;
        }

        conn.checkpoint()?;
        Ok(())
    }
}
