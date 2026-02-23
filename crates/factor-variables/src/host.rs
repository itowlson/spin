use spin_core::wasmtime::{self, component::Accessor};
use spin_factors::anyhow;
use spin_telemetry::traces::{self, Blame};
use spin_world::{v1, v2::variables, wasi::config as wasi_config, spin::variables::variables as v3};
use tracing::instrument;

use crate::InstanceState;

impl variables::Host for InstanceState {
    #[instrument(name = "spin_variables.get", skip(self), fields(otel.kind = "client"))]
    async fn get(&mut self, key: String) -> Result<String, variables::Error> {
        self.otel.reparent_tracing_span();
        let key = spin_expressions::Key::new(&key).map_err(expressions_to_variables_err)?;
        self.expression_resolver
            .resolve(&self.component_id, key)
            .await
            .map_err(expressions_to_variables_err)
    }

    fn convert_error(&mut self, error: variables::Error) -> anyhow::Result<variables::Error> {
        Ok(error)
    }
}

impl v1::config::Host for InstanceState {
    #[instrument(name = "spin_config.get", skip(self), fields(otel.kind = "client"))]
    async fn get_config(&mut self, key: String) -> Result<String, v1::config::Error> {
        <Self as variables::Host>::get(self, key)
            .await
            .map_err(|err| match err {
                variables::Error::InvalidName(msg) => v1::config::Error::InvalidKey(msg),
                variables::Error::Undefined(msg) => v1::config::Error::Provider(msg),
                other => v1::config::Error::Other(format!("{other}")),
            })
    }

    fn convert_error(&mut self, err: v1::config::Error) -> anyhow::Result<v1::config::Error> {
        Ok(err)
    }
}

impl wasi_config::store::Host for InstanceState {
    #[instrument(name = "wasi_config.get", skip(self), fields(otel.kind = "client"))]
    async fn get(&mut self, key: String) -> Result<Option<String>, wasi_config::store::Error> {
        match <Self as variables::Host>::get(self, key).await {
            Ok(value) => Ok(Some(value)),
            Err(variables::Error::Undefined(_)) => Ok(None),
            Err(variables::Error::InvalidName(_)) => Ok(None), // this is the guidance from https://github.com/WebAssembly/wasi-runtime-config/pull/19)
            Err(variables::Error::Provider(msg)) => Err(wasi_config::store::Error::Upstream(msg)),
            Err(variables::Error::Other(msg)) => Err(wasi_config::store::Error::Io(msg)),
        }
    }

    #[instrument(name = "wasi_config.get_all", skip(self), fields(otel.kind = "client"))]
    async fn get_all(&mut self) -> Result<Vec<(String, String)>, wasi_config::store::Error> {
        let all = self
            .expression_resolver
            .resolve_all(&self.component_id)
            .await;
        all.map_err(|e| {
            match expressions_to_variables_err(e) {
                variables::Error::Undefined(msg) => wasi_config::store::Error::Io(msg), // this shouldn't happen but just in case
                variables::Error::InvalidName(msg) => wasi_config::store::Error::Io(msg), // this shouldn't happen but just in case
                variables::Error::Provider(msg) => wasi_config::store::Error::Upstream(msg),
                variables::Error::Other(msg) => wasi_config::store::Error::Io(msg),
            }
        })
    }

    fn convert_error(
        &mut self,
        err: wasi_config::store::Error,
    ) -> anyhow::Result<wasi_config::store::Error> {
        Ok(err)
    }
}

impl v3::Host for InstanceState {
    fn convert_error(&mut self, err: v3::Error) -> wasmtime::Result<v3::Error> {
        Ok(err)
    }
}

impl v3::HostWithStore for super::VariablesFactorData {
    async fn get<T>(accessor: &Accessor<T, Self>, key: String) -> Result<String, v3::Error> {
        let (resolver, component_id) = accessor.with(|mut access| {
            let host = access.get();
            host.otel.reparent_tracing_span();
            (host.expression_resolver.clone(), host.component_id.clone())
        });

        let key = spin_expressions::Key::new(&key).map_err(expressions_to_variables_err_v3)?;

        resolver.resolve(&component_id, key).await.map_err(expressions_to_variables_err_v3)
    }
}

/// Convert a `spin_expressions::Error` to a `variables::Error`, setting the current span's status and fault attribute.
fn expressions_to_variables_err(err: spin_expressions::Error) -> variables::Error {
    use spin_expressions::Error;
    let blame = match err {
        Error::InvalidName(_) | Error::InvalidTemplate(_) | Error::Undefined(_) => Blame::Guest,
        Error::Provider(_) => Blame::Host,
    };
    traces::mark_as_error(&err, Some(blame));
    match err {
        Error::InvalidName(msg) => variables::Error::InvalidName(msg),
        Error::Undefined(msg) => variables::Error::Undefined(msg),
        Error::InvalidTemplate(_) => variables::Error::Other(format!("{err}")),
        Error::Provider(err) => variables::Error::Provider(err.to_string()),
    }
}

/// Convert a `spin_expressions::Error` to a `variables::Error`, setting the current span's status and fault attribute.
fn expressions_to_variables_err_v3(err: spin_expressions::Error) -> v3::Error {
    use spin_expressions::Error;
    let blame = match err {
        Error::InvalidName(_) | Error::InvalidTemplate(_) | Error::Undefined(_) => Blame::Guest,
        Error::Provider(_) => Blame::Host,
    };
    traces::mark_as_error(&err, Some(blame));
    match err {
        Error::InvalidName(msg) => v3::Error::InvalidName(msg),
        Error::Undefined(msg) => v3::Error::Undefined(msg),
        Error::InvalidTemplate(_) => v3::Error::Other(format!("{err}")),
        Error::Provider(err) => v3::Error::Provider(err.to_string()),
    }
}
