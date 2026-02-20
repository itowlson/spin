use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use spin_factor_sqlite::Connection;
use spin_world::spin::sqlite3_1_0::sqlite as v3;
use spin_world::spin::sqlite3_1_0::sqlite::{self, RowResult};
use tokio::sync::OnceCell;

/// A lazy wrapper around a [`LibSqlConnection`] that implements the [`Connection`] trait.
pub struct LazyLibSqlConnection {
    url: String,
    token: String,
    // Since the libSQL client can only be created asynchronously, we wait until
    // we're in the `Connection` implementation to create. Since we only want to do
    // this once, we use a `OnceCell` to store it.
    inner: OnceCell<Arc<LibSqlConnection>>,
}

impl LazyLibSqlConnection {
    pub fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            inner: OnceCell::new(),
        }
    }

    pub async fn get_or_create_connection(&self) -> Result<&Arc<LibSqlConnection>, v3::Error> {
        self.inner
            .get_or_try_init(|| async {
                LibSqlConnection::create(self.url.clone(), self.token.clone())
                    .await
                    .context("failed to create SQLite client")
                    .map(Arc::new)
            })
            .await
            .map_err(|_| v3::Error::InvalidConnection)
    }
}

#[async_trait]
impl Connection for LazyLibSqlConnection {
    async fn query(
        &self,
        query: &str,
        parameters: Vec<v3::Value>,
    ) -> Result<v3::QueryResult, v3::Error> {
        let client = self.get_or_create_connection().await?;
        client.query(query, parameters).await
    }

    async fn query_async(
        &self,
        query: &str,
        parameters: Vec<v3::Value>,
    ) -> Result<(tokio::sync::oneshot::Receiver<Vec<String>>, tokio::sync::mpsc::Receiver<Result<v3::RowResult, v3::Error>>), v3::Error> {
        let client = self.get_or_create_connection().await?.clone();
        let query = query.to_string();

        let (cols_tx, cols_rx) = tokio::sync::oneshot::channel();
        let (rows_tx, rows_rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let result = client
                .inner
                .query(&query, convert_parameters(&parameters))
                .await
                .map_err(|e| sqlite::Error::Io(e.to_string()));

            let mut rows = match result {
                Ok(r) => r,
                Err(e) => {
                    rows_tx.send(Err(e)).await.unwrap();
                    return;
                }
            };

            let columns = columns(&rows);
            cols_tx.send(columns).unwrap();

            let column_count = rows.column_count();

            loop {
                let row = match rows.next().await {
                    Ok(Some(r)) => r,
                    Ok(None) => break,
                    Err(e) => {
                        rows_tx.send(Err(v3::Error::Io(e.to_string()))).await.unwrap();
                        break;
                    }
                };
                let row = convert_row(row, column_count);
                rows_tx.send(Ok(row)).await.unwrap();
            }
        });

        Ok((cols_rx, rows_rx))
    }

    async fn execute_batch(&self, statements: &str) -> anyhow::Result<()> {
        let client = self.get_or_create_connection().await?;
        client.execute_batch(statements).await
    }

    async fn changes(&self) -> Result<u64, sqlite::Error> {
        let client = self.get_or_create_connection().await?;
        Ok(client.changes())
    }

    async fn last_insert_rowid(&self) -> Result<i64, sqlite::Error> {
        let client = self.get_or_create_connection().await?;
        Ok(client.last_insert_rowid())
    }

    fn summary(&self) -> Option<String> {
        Some(format!("libSQL at {}", self.url))
    }
}

/// An open connection to a libSQL server.
#[derive(Clone)]
pub struct LibSqlConnection {
    inner: libsql::Connection,
}

impl LibSqlConnection {
    pub async fn create(url: String, token: String) -> anyhow::Result<Self> {
        let db = libsql::Builder::new_remote(url, token).build().await?;
        let inner = db.connect()?;
        Ok(Self { inner })
    }
}

impl LibSqlConnection {
    pub async fn query(
        &self,
        query: &str,
        parameters: Vec<sqlite::Value>,
    ) -> Result<sqlite::QueryResult, sqlite::Error> {
        let result = self
            .inner
            .query(query, convert_parameters(&parameters))
            .await
            .map_err(|e| sqlite::Error::Io(e.to_string()))?;

        Ok(sqlite::QueryResult {
            columns: columns(&result),
            rows: convert_rows(result)
                .await
                .map_err(|e| sqlite::Error::Io(e.to_string()))?,
        })
    }

    pub async fn execute_batch(&self, statements: &str) -> anyhow::Result<()> {
        self.inner.execute_batch(statements).await?;

        Ok(())
    }

    pub fn changes(&self) -> u64 {
        self.inner.changes()
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.inner.last_insert_rowid()
    }
}

fn columns(rows: &libsql::Rows) -> Vec<String> {
    (0..rows.column_count())
        .map(|index| rows.column_name(index).unwrap_or("").to_owned())
        .collect()
}

async fn convert_rows(mut rows: libsql::Rows) -> anyhow::Result<Vec<RowResult>> {
    let mut result_rows = vec![];

    let column_count = rows.column_count();

    while let Some(row) = rows.next().await? {
        result_rows.push(convert_row(row, column_count));
    }

    Ok(result_rows)
}

fn convert_row(row: libsql::Row, column_count: i32) -> RowResult {
    let values = (0..column_count)
        .map(|index| convert_value(row.get_value(index).unwrap()))
        .collect();
    RowResult { values }
}

fn convert_value(v: libsql::Value) -> sqlite::Value {
    use libsql::Value;

    match v {
        Value::Null => sqlite::Value::Null,
        Value::Integer(value) => sqlite::Value::Integer(value),
        Value::Real(value) => sqlite::Value::Real(value),
        Value::Text(value) => sqlite::Value::Text(value),
        Value::Blob(value) => sqlite::Value::Blob(value),
    }
}

fn convert_parameters(parameters: &[sqlite::Value]) -> Vec<libsql::Value> {
    use libsql::Value;

    parameters
        .iter()
        .map(|v| match v {
            sqlite::Value::Integer(value) => Value::Integer(*value),
            sqlite::Value::Real(value) => Value::Real(*value),
            sqlite::Value::Text(t) => Value::Text(t.clone()),
            sqlite::Value::Blob(b) => Value::Blob(b.clone()),
            sqlite::Value::Null => Value::Null,
        })
        .collect()
}
