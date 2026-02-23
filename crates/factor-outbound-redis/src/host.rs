use std::net::SocketAddr;

use anyhow::Result;
use redis::io::AsyncDNSResolver;
use redis::AsyncConnectionConfig;
use redis::{aio::MultiplexedConnection, AsyncCommands, FromRedisValue, Value};
use spin_core::wasmtime;
use spin_core::wasmtime::component::{Accessor, Resource};
use spin_factor_otel::OtelFactorState;
use spin_factor_outbound_networking::config::allowed_hosts::OutboundAllowedHosts;
use spin_factor_outbound_networking::config::blocked_networks::BlockedNetworks;
use spin_world::v1::{redis as v1, redis_types};
use spin_world::v2::redis::{
    self as v2, Connection as RedisConnection, Error, RedisParameter, RedisResult,
};
use super::v3;
use tracing::field::Empty;
use tracing::{instrument, Level};

pub struct InstanceState {
    pub allowed_hosts: OutboundAllowedHosts,
    pub blocked_networks: BlockedNetworks,
    pub connections: spin_resource_table::Table<MultiplexedConnection>,
    pub otel: OtelFactorState,
}

impl InstanceState {
    async fn is_address_allowed(&self, address: &str) -> Result<bool> {
        self.allowed_hosts.check_url(address, "redis").await
    }

    async fn establish_connection(
        &mut self,
        address: String,
    ) -> Result<Resource<RedisConnection>, Error> {
        let config = AsyncConnectionConfig::new()
            .set_dns_resolver(SpinDnsResolver(self.blocked_networks.clone()));
        let conn = redis::Client::open(address.as_str())
            .map_err(|_| Error::InvalidAddress)?
            .get_multiplexed_async_connection_with_config(&config)
            .await
            .map_err(other_error)?;
        self.connections
            .push(conn)
            .map(Resource::new_own)
            .map_err(|_| Error::TooManyConnections)
    }

    async fn get_conn(
        &mut self,
        connection: Resource<RedisConnection>,
    ) -> Result<&mut MultiplexedConnection, Error> {
        self.connections
            .get_mut(connection.rep())
            .ok_or(Error::Other(
                "could not find connection for resource".into(),
            ))
    }

    fn get_conn_v3(
        &mut self,
        connection: Resource<v3::Connection>,
    ) -> Result<MultiplexedConnection, v3::Error> {
        self.connections
            .get_mut(connection.rep())
            .ok_or(v3::Error::Other(
                "could not find connection for resource".into(),
            ))
            .cloned()
    }
}

impl v2::Host for crate::InstanceState {
    fn convert_error(&mut self, error: Error) -> Result<Error> {
        Ok(error)
    }
}

impl v2::HostConnection for crate::InstanceState {
    #[instrument(name = "spin_outbound_redis.open_connection", skip(self, address), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", db.address = Empty, server.port = Empty, db.namespace = Empty))]
    async fn open(&mut self, address: String) -> Result<Resource<RedisConnection>, Error> {
        self.otel.reparent_tracing_span();
        if !self
            .is_address_allowed(&address)
            .await
            .map_err(|e| v2::Error::Other(e.to_string()))?
        {
            return Err(Error::InvalidAddress);
        }

        self.establish_connection(address).await
    }

    #[instrument(name = "spin_outbound_redis.publish", skip(self, connection, payload), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("PUBLISH {}", channel)))]
    async fn publish(
        &mut self,
        connection: Resource<RedisConnection>,
        channel: String,
        payload: Vec<u8>,
    ) -> Result<(), Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        // The `let () =` syntax is needed to suppress a warning when the result type is inferred.
        // You can read more about the issue here: <https://github.com/redis-rs/redis-rs/issues/1228>
        let () = conn
            .publish(&channel, &payload)
            .await
            .map_err(other_error)?;
        Ok(())
    }

    #[instrument(name = "spin_outbound_redis.get", skip(self, connection), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("GET {}", key)))]
    async fn get(
        &mut self,
        connection: Resource<RedisConnection>,
        key: String,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        let value = conn.get(&key).await.map_err(other_error)?;
        Ok(value)
    }

    #[instrument(name = "spin_outbound_redis.set", skip(self, connection, value), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("SET {}", key)))]
    async fn set(
        &mut self,
        connection: Resource<RedisConnection>,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        // The `let () =` syntax is needed to suppress a warning when the result type is inferred.
        // You can read more about the issue here: <https://github.com/redis-rs/redis-rs/issues/1228>
        let () = conn.set(&key, &value).await.map_err(other_error)?;
        Ok(())
    }

    #[instrument(name = "spin_outbound_redis.incr", skip(self, connection), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("INCRBY {} 1", key)))]
    async fn incr(
        &mut self,
        connection: Resource<RedisConnection>,
        key: String,
    ) -> Result<i64, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        let value = conn.incr(&key, 1).await.map_err(other_error)?;
        Ok(value)
    }

    #[instrument(name = "spin_outbound_redis.del", skip(self, connection), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("DEL {}", keys.join(" "))))]
    async fn del(
        &mut self,
        connection: Resource<RedisConnection>,
        keys: Vec<String>,
    ) -> Result<u32, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        let value = conn.del(&keys).await.map_err(other_error)?;
        Ok(value)
    }

    #[instrument(name = "spin_outbound_redis.sadd", skip(self, connection, values), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("SADD {} {}", key, values.join(" "))))]
    async fn sadd(
        &mut self,
        connection: Resource<RedisConnection>,
        key: String,
        values: Vec<String>,
    ) -> Result<u32, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        let value = conn.sadd(&key, &values).await.map_err(|e| {
            if e.kind() == redis::ErrorKind::TypeError {
                Error::TypeError
            } else {
                Error::Other(e.to_string())
            }
        })?;
        Ok(value)
    }

    #[instrument(name = "spin_outbound_redis.smembers", skip(self, connection), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("SMEMBERS {}", key)))]
    async fn smembers(
        &mut self,
        connection: Resource<RedisConnection>,
        key: String,
    ) -> Result<Vec<String>, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        let value = conn.smembers(&key).await.map_err(other_error)?;
        Ok(value)
    }

    #[instrument(name = "spin_outbound_redis.srem", skip(self, connection, values), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("SREM {} {}", key, values.join(" "))))]
    async fn srem(
        &mut self,
        connection: Resource<RedisConnection>,
        key: String,
        values: Vec<String>,
    ) -> Result<u32, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await.map_err(other_error)?;
        let value = conn.srem(&key, &values).await.map_err(other_error)?;
        Ok(value)
    }

    #[instrument(name = "spin_outbound_redis.execute", skip(self, connection), err(level = Level::INFO), fields(otel.kind = "client", db.system = "redis", otel.name = format!("{}", command)))]
    async fn execute(
        &mut self,
        connection: Resource<RedisConnection>,
        command: String,
        arguments: Vec<RedisParameter>,
    ) -> Result<Vec<RedisResult>, Error> {
        self.otel.reparent_tracing_span();

        let conn = self.get_conn(connection).await?;
        let mut cmd = redis::cmd(&command);
        arguments.iter().for_each(|value| match value {
            RedisParameter::Int64(v) => {
                cmd.arg(v);
            }
            RedisParameter::Binary(v) => {
                cmd.arg(v);
            }
        });

        cmd.query_async::<RedisResults>(conn)
            .await
            .map(|values| values.0)
            .map_err(other_error)
    }

    async fn drop(&mut self, connection: Resource<RedisConnection>) -> anyhow::Result<()> {
        self.connections.remove(connection.rep());
        Ok(())
    }
}

impl v3::Host for InstanceState {
    fn convert_error(&mut self, err: v3::Error) -> anyhow::Result<v3::Error> {
        Ok(err)
    }
}

impl v3::HostConnection for InstanceState {
    async fn drop(&mut self, connection: Resource<v3::Connection>) -> anyhow::Result<()> {
        self.connections.remove(connection.rep());
        Ok(())
    }
}

impl v3::HostConnectionWithStore for super::RedisFactorData {
    async fn open<T>(accessor: &Accessor<T, Self>, address: String) -> Result<Resource<v3::Connection>, v3::Error> {
        let (allowed_hosts, blocked_networks) = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            (host.allowed_hosts.clone(), host.blocked_networks.clone())
        });

        if !allowed_hosts.check_url(&address, "redis").await.map_err(|e| v3::Error::Other(e.to_string()))? {
            return Err(v3::Error::InvalidAddress);
        }

        let config = AsyncConnectionConfig::new()
            .set_dns_resolver(SpinDnsResolver(blocked_networks));
        let conn = redis::Client::open(address.as_str())
            .map_err(|_| v3::Error::InvalidAddress)?
            .get_multiplexed_async_connection_with_config(&config)
            .await
            .map_err(other_error_v3)?;

        let resource = accessor.with(|mut access| {
            let host = access.get();
            host.connections
                .push(conn)
                .map(Resource::new_own)
                .map_err(|_| v3::Error::TooManyConnections)
        })?;

        Ok(resource)
    }

    async fn publish<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,channel:String,payload:v3::Payload,) -> Result<(),v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        // The `let () =` syntax is needed to suppress a warning when the result type is inferred.
        // You can read more about the issue here: <https://github.com/redis-rs/redis-rs/issues/1228>
        let () = conn
            .publish(&channel, &payload)
            .await
            .map_err(other_error_v3)?;
        Ok(())
    }

    async fn get<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,key:String,) -> Result<Option<v3::Payload>,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let value = conn.get(&key).await.map_err(other_error_v3)?;
        Ok(value)
    }

    async fn set<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,key:String,value:v3::Payload,) -> Result<(),v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        // The `let () =` syntax is needed to suppress a warning when the result type is inferred.
        // You can read more about the issue here: <https://github.com/redis-rs/redis-rs/issues/1228>
        let () = conn.set(&key, &value).await.map_err(other_error_v3)?;
        Ok(())
    }

    async fn incr<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,key:String,) -> Result<i64,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let value = conn.incr(&key, 1).await.map_err(other_error_v3)?;
        Ok(value)
    }

    async fn del<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,keys:Vec<String>,) -> Result<u32,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let value = conn.del(&keys).await.map_err(other_error_v3)?;
        Ok(value)
    }

    async fn sadd<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,key:String,values:Vec<String>,) -> Result<u32,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let value = conn.sadd(&key, &values).await.map_err(|e| {
            if e.kind() == redis::ErrorKind::TypeError {
                v3::Error::TypeError
            } else {
                v3::Error::Other(e.to_string())
            }
        })?;
        Ok(value)
    }

    async fn smembers<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,key:String,) -> Result<wasmtime::component::__internal::Vec<String>,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let value = conn.smembers(&key).await.map_err(other_error_v3)?;
        Ok(value)
    }

    async fn srem<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,key:String,values:Vec<String>,) -> Result<u32,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let value = conn.srem(&key, &values).await.map_err(other_error_v3)?;
        Ok(value)
    }

    async fn execute<T>(accessor: &Accessor<T,Self>, connection:Resource<v3::Connection>,command:String,arguments:Vec<v3::RedisParameter>,) -> Result<Vec<v3::RedisResult>,v3::Error> {
        let mut conn = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            host.get_conn_v3(connection)
        })?;

        let mut cmd = redis::cmd(&command);
        arguments.iter().for_each(|value| match value {
            v3::RedisParameter::Int64(v) => {
                cmd.arg(v);
            }
            v3::RedisParameter::Binary(v) => {
                cmd.arg(v);
            }
        });

        cmd.query_async::<RedisResultsV3>(&mut conn)
            .await
            .map(|values| values.0)
            .map_err(other_error_v3)
    }
}

fn other_error(e: impl std::fmt::Display) -> Error {
    Error::Other(e.to_string())
}

fn other_error_v3(e: impl std::fmt::Display) -> v3::Error {
    v3::Error::Other(e.to_string())
}

/// Delegate a function call to the v2::HostConnection implementation
macro_rules! delegate {
    ($self:ident.$name:ident($address:expr, $($arg:expr),*)) => {{
        if !$self.is_address_allowed(&$address).await.map_err(|_| v1::Error::Error)?  {
            return Err(v1::Error::Error);
        }
        let connection = match $self.establish_connection($address).await {
            Ok(c) => c,
            Err(_) => return Err(v1::Error::Error),
        };
        <Self as v2::HostConnection>::$name($self, connection, $($arg),*)
            .await
            .map_err(|_| v1::Error::Error)
    }};
}

impl v1::Host for crate::InstanceState {
    async fn publish(
        &mut self,
        address: String,
        channel: String,
        payload: Vec<u8>,
    ) -> Result<(), v1::Error> {
        delegate!(self.publish(address, channel, payload))
    }

    async fn get(&mut self, address: String, key: String) -> Result<Vec<u8>, v1::Error> {
        delegate!(self.get(address, key)).map(|v| v.unwrap_or_default())
    }

    async fn set(&mut self, address: String, key: String, value: Vec<u8>) -> Result<(), v1::Error> {
        delegate!(self.set(address, key, value))
    }

    async fn incr(&mut self, address: String, key: String) -> Result<i64, v1::Error> {
        delegate!(self.incr(address, key))
    }

    async fn del(&mut self, address: String, keys: Vec<String>) -> Result<i64, v1::Error> {
        delegate!(self.del(address, keys)).map(|v| v as i64)
    }

    async fn sadd(
        &mut self,
        address: String,
        key: String,
        values: Vec<String>,
    ) -> Result<i64, v1::Error> {
        delegate!(self.sadd(address, key, values)).map(|v| v as i64)
    }

    async fn smembers(&mut self, address: String, key: String) -> Result<Vec<String>, v1::Error> {
        delegate!(self.smembers(address, key))
    }

    async fn srem(
        &mut self,
        address: String,
        key: String,
        values: Vec<String>,
    ) -> Result<i64, v1::Error> {
        delegate!(self.srem(address, key, values)).map(|v| v as i64)
    }

    async fn execute(
        &mut self,
        address: String,
        command: String,
        arguments: Vec<v1::RedisParameter>,
    ) -> Result<Vec<v1::RedisResult>, v1::Error> {
        delegate!(self.execute(
            address,
            command,
            arguments.into_iter().map(Into::into).collect()
        ))
        .map(|v| v.into_iter().map(Into::into).collect())
    }
}

impl redis_types::Host for crate::InstanceState {
    fn convert_error(&mut self, error: redis_types::Error) -> Result<redis_types::Error> {
        Ok(error)
    }
}

struct RedisResults(Vec<RedisResult>);
struct RedisResultsV3(Vec<v3::RedisResult>);

impl FromRedisValue for RedisResults {
    fn from_redis_value(value: &Value) -> redis::RedisResult<Self> {
        fn append(values: &mut Vec<RedisResult>, value: &Value) -> redis::RedisResult<()> {
            match value {
                Value::Nil => {
                    values.push(RedisResult::Nil);
                    Ok(())
                }
                Value::Int(v) => {
                    values.push(RedisResult::Int64(*v));
                    Ok(())
                }
                Value::BulkString(bytes) => {
                    values.push(RedisResult::Binary(bytes.to_owned()));
                    Ok(())
                }
                Value::SimpleString(s) => {
                    values.push(RedisResult::Status(s.to_owned()));
                    Ok(())
                }
                Value::Okay => {
                    values.push(RedisResult::Status("OK".to_string()));
                    Ok(())
                }
                Value::Map(_) => Err(redis::RedisError::from((
                    redis::ErrorKind::TypeError,
                    "Could not convert Redis response",
                    "Redis Map type is not supported".to_string(),
                ))),
                Value::Attribute { .. } => Err(redis::RedisError::from((
                    redis::ErrorKind::TypeError,
                    "Could not convert Redis response",
                    "Redis Attribute type is not supported".to_string(),
                ))),
                Value::Array(arr) | Value::Set(arr) => {
                    arr.iter().try_for_each(|value| append(values, value))
                }
                Value::Double(v) => {
                    values.push(RedisResult::Binary(v.to_string().into_bytes()));
                    Ok(())
                }
                Value::VerbatimString { .. } => Err(redis::RedisError::from((
                    redis::ErrorKind::TypeError,
                    "Could not convert Redis response",
                    "Redis string with format attribute is not supported".to_string(),
                ))),
                Value::Boolean(v) => {
                    values.push(RedisResult::Int64(if *v { 1 } else { 0 }));
                    Ok(())
                }
                Value::BigNumber(v) => {
                    values.push(RedisResult::Binary(v.to_string().as_bytes().to_owned()));
                    Ok(())
                }
                Value::Push { .. } => Err(redis::RedisError::from((
                    redis::ErrorKind::TypeError,
                    "Could not convert Redis response",
                    "Redis Pub/Sub types are not supported".to_string(),
                ))),
                Value::ServerError(err) => Err(redis::RedisError::from((
                    redis::ErrorKind::ResponseError,
                    "Server error",
                    format!("{err:?}"),
                ))),
            }
        }
        let mut values = Vec::new();
        append(&mut values, value)?;
        Ok(RedisResults(values))
    }
}

impl FromRedisValue for RedisResultsV3 {
    fn from_redis_value(v: &Value) -> redis::RedisResult<Self> {
        let results = <RedisResults as FromRedisValue>::from_redis_value(v)?;
        Ok(Self(results.0.into_iter().map(v2_value_to_v3).collect()))
    }
}

fn v2_value_to_v3(v: RedisResult) -> v3::RedisResult {
    match v {
        RedisResult::Nil => v3::RedisResult::Nil,
        RedisResult::Status(v) => v3::RedisResult::Status(v),
        RedisResult::Int64(v) => v3::RedisResult::Int64(v),
        RedisResult::Binary(v) => v3::RedisResult::Binary(v),
    }
}

/// Resolves DNS using Tokio's resolver, filtering out blocked IPs.
struct SpinDnsResolver(BlockedNetworks);

impl AsyncDNSResolver for SpinDnsResolver {
    fn resolve<'a, 'b: 'a>(
        &'a self,
        host: &'b str,
        port: u16,
    ) -> redis::RedisFuture<'a, Box<dyn Iterator<Item = std::net::SocketAddr> + Send + 'a>> {
        Box::pin(async move {
            let mut addrs = tokio::net::lookup_host((host, port))
                .await?
                .collect::<Vec<_>>();
            // Remove blocked IPs
            let blocked_addrs = self.0.remove_blocked(&mut addrs);
            if addrs.is_empty() && !blocked_addrs.is_empty() {
                tracing::error!(
                    "error.type" = "destination_ip_prohibited",
                    ?blocked_addrs,
                    "all destination IP(s) prohibited by runtime config"
                );
            }
            Ok(Box::new(addrs.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>)
        })
    }
}
