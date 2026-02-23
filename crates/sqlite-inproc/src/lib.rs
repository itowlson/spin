use std::{
    path::PathBuf,
    sync::OnceLock,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use async_trait::async_trait;
use spin_factor_sqlite::Connection;
use spin_world::spin::sqlite3_1_0::sqlite;
use spin_world::spin::sqlite3_1_0::sqlite::{self as v3};

/// The location of an in-process sqlite database.
#[derive(Debug, Clone)]
pub enum InProcDatabaseLocation {
    /// An in-memory sqlite database.
    InMemory,
    /// The path to the sqlite database.
    Path(PathBuf),
}

impl InProcDatabaseLocation {
    /// Convert an optional path to a database location.
    ///
    /// Ensures that the parent directory of the database exists. If path is None, then an in memory
    /// database will be used.
    pub fn from_path(path: Option<PathBuf>) -> anyhow::Result<Self> {
        match path {
            Some(path) => {
                // Create the store's parent directory if necessary
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!(
                            "failed to create sqlite database directory '{}'",
                            parent.display()
                        )
                    })?;
                }
                Ok(Self::Path(path))
            }
            None => Ok(Self::InMemory),
        }
    }
}

/// A connection to a sqlite database
pub struct InProcConnection {
    location: InProcDatabaseLocation,
    connection: OnceLock<Arc<Mutex<rusqlite::Connection>>>,
}

impl InProcConnection {
    pub fn new(location: InProcDatabaseLocation) -> Result<Self, sqlite::Error> {
        let connection = OnceLock::new();
        Ok(Self {
            location,
            connection,
        })
    }

    pub fn db_connection(&self) -> Result<Arc<Mutex<rusqlite::Connection>>, sqlite::Error> {
        if let Some(c) = self.connection.get() {
            return Ok(c.clone());
        }
        // Only create the connection if we failed to get it.
        // We might do duplicate work here if there's a race, but that's fine.
        let new = self.create_connection()?;
        Ok(self.connection.get_or_init(|| new)).cloned()
    }

    fn create_connection(&self) -> Result<Arc<Mutex<rusqlite::Connection>>, sqlite::Error> {
        let connection = match &self.location {
            InProcDatabaseLocation::InMemory => rusqlite::Connection::open_in_memory(),
            InProcDatabaseLocation::Path(path) => rusqlite::Connection::open(path),
        }
        .map_err(|e| sqlite::Error::Io(e.to_string()))?;
        Ok(Arc::new(Mutex::new(connection)))
    }
}

#[async_trait]
impl Connection for InProcConnection {
    async fn query(
        &self,
        query: &str,
        parameters: Vec<sqlite::Value>,
    ) -> Result<sqlite::QueryResult, sqlite::Error> {
        let connection = self.db_connection()?;
        let query = query.to_owned();
        // Tell the tokio runtime that we're going to block while making the query
        tokio::task::spawn_blocking(move || execute_query(&connection, &query, parameters))
            .await
            .context("internal runtime error")
            .map_err(|e| sqlite::Error::Io(e.to_string()))?
    }

    async fn query_async(
        &self,
        query: &str,
        parameters: Vec<v3::Value>,
    ) -> Result<(tokio::sync::oneshot::Receiver<Vec<String>>, tokio::sync::mpsc::Receiver<Result<v3::RowResult, v3::Error>>), v3::Error> {
        let connection = self.db_connection()?;
        let query = query.to_owned();

        // let conn = connection.lock().unwrap();
        // let mut statement = conn
        //     .prepare_cached(&query)
        //     .map_err(|e| sqlite::Error::Io(e.to_string()))?;
        // let columns = statement
        //     .column_names()
        //     .into_iter()
        //     .map(ToOwned::to_owned)
        //     .collect();

        let (cols_tx, cols_rx) = tokio::sync::oneshot::channel();
        // let (rows_tx, rows_rx) = tokio::sync::mpsc::channel(100);
        let (rows_sync_tx, rows_sync_rx) = std::sync::mpsc::channel();

        tokio::spawn(async move {
            let conn = connection.lock().unwrap();
            let mut statement = match conn
                .prepare_cached(&query) {
                    Ok(s) => s,
                    Err(e) => {
                        rows_sync_tx.send(Err(sqlite::Error::Io(e.to_string()))).unwrap();
                        return;
                    }
                };
            let columns: Vec<_> = statement
                .column_names()
                .into_iter()
                .map(ToOwned::to_owned)
                .collect();
            cols_tx.send(columns).unwrap();

            let rows = statement
                .query(rusqlite::params_from_iter(convert_data(parameters.into_iter())));

            let mut rows = match rows {
                Err(e) => {
                    rows_sync_tx.send(Err(sqlite::Error::Io(e.to_string()))).unwrap();
                    return;
                },
                Ok(r) => r,
            };

            loop {
                let row = match rows.next() {
                    Err(e) => {
                        rows_sync_tx.send(Err(v3::Error::Io(e.to_string()))).unwrap();
                        break;
                    }
                    Ok(None) => break,
                    Ok(Some(r)) => r,
                };

                match convert_row(&row) {
                    Ok(row) => rows_sync_tx.send(Ok(row)).unwrap(),
                    Err(e) => {
                        let err = v3::Error::Io(e.to_string());
                        rows_sync_tx.send(Err(err)).unwrap();
                    }
                }
            }
        });
        
        let rows_rx = spin_wasi_async::stream::asyncify(rows_sync_rx);
        // tokio::spawn(async move {
        //     loop {
        //         match rows_sync_rx.recv_timeout(tokio::time::Duration::from_millis(1)) {
        //             Ok(r) => rows_tx.send(r).await.unwrap(),
        //             Err(std::sync::mpsc::RecvTimeoutError::Timeout) => { tokio::task::yield_now().await; }
        //             Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        //         }
        //     }
            
        // });

        Ok((cols_rx, rows_rx))
    }

    async fn execute_batch(&self, statements: &str) -> anyhow::Result<()> {
        let connection = self.db_connection()?;
        let statements = statements.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = connection.lock().unwrap();
            conn.execute_batch(&statements)
                .context("failed to execute batch statements")
        })
        .await?
        .context("failed to spawn blocking task")?;
        Ok(())
    }

    async fn changes(&self) -> Result<u64, sqlite::Error> {
        let connection = self.db_connection()?;
        let conn = connection.lock().unwrap();
        Ok(conn.changes())
    }

    async fn last_insert_rowid(&self) -> Result<i64, sqlite::Error> {
        let connection = self.db_connection()?;
        let conn = connection.lock().unwrap();
        Ok(conn.last_insert_rowid())
    }

    fn summary(&self) -> Option<String> {
        Some(match &self.location {
            InProcDatabaseLocation::InMemory => "a temporary in-memory database".to_string(),
            InProcDatabaseLocation::Path(path) => format!("\"{}\"", path.display()),
        })
    }
}

fn convert_row(row: &rusqlite::Row) -> Result<sqlite::RowResult, rusqlite::Error> {
    let mut values = vec![];
    for column in 0.. {
        let value = row.get::<usize, ValueWrapper>(column);
        if let Err(rusqlite::Error::InvalidColumnIndex(_)) = value {
            break;
        }
        let value = value?.0;
        values.push(value);
    }
    Ok(sqlite::RowResult { values })
}

// This function lives outside the query function to make it more readable.
fn execute_query(
    connection: &Mutex<rusqlite::Connection>,
    query: &str,
    parameters: Vec<sqlite::Value>,
) -> Result<sqlite::QueryResult, sqlite::Error> {
    let conn = connection.lock().unwrap();
    let mut statement = conn
        .prepare_cached(query)
        .map_err(|e| sqlite::Error::Io(e.to_string()))?;
    let columns = statement
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    let rows = statement
        .query_map(
            rusqlite::params_from_iter(convert_data(parameters.into_iter())),
            convert_row,
        )
        .map_err(|e| sqlite::Error::Io(e.to_string()))?;
    let rows = rows
        .into_iter()
        .map(|r| r.map_err(|e| sqlite::Error::Io(e.to_string())))
        .collect::<Result<_, sqlite::Error>>()?;
    Ok(sqlite::QueryResult { columns, rows })
}

fn convert_data(
    arguments: impl Iterator<Item = sqlite::Value>,
) -> impl Iterator<Item = rusqlite::types::Value> {
    arguments.map(|a| match a {
        sqlite::Value::Null => rusqlite::types::Value::Null,
        sqlite::Value::Integer(i) => rusqlite::types::Value::Integer(i),
        sqlite::Value::Real(r) => rusqlite::types::Value::Real(r),
        sqlite::Value::Text(t) => rusqlite::types::Value::Text(t),
        sqlite::Value::Blob(b) => rusqlite::types::Value::Blob(b),
    })
}

// A wrapper around sqlite::Value so that we can convert from rusqlite ValueRef
struct ValueWrapper(sqlite::Value);

impl rusqlite::types::FromSql for ValueWrapper {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let value = match value {
            rusqlite::types::ValueRef::Null => sqlite::Value::Null,
            rusqlite::types::ValueRef::Integer(i) => sqlite::Value::Integer(i),
            rusqlite::types::ValueRef::Real(f) => sqlite::Value::Real(f),
            rusqlite::types::ValueRef::Text(t) => {
                sqlite::Value::Text(String::from_utf8(t.to_vec()).unwrap())
            }
            rusqlite::types::ValueRef::Blob(b) => sqlite::Value::Blob(b.to_vec()),
        };
        Ok(ValueWrapper(value))
    }
}
