mod host;
pub mod runtime_config;
mod util;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::ensure;
use spin_factor_otel::OtelFactorState;
use spin_factors::{
    ConfigureAppContext, Factor, FactorData, FactorInstanceBuilder, InitContext, PrepareContext,
    RuntimeFactors,
};
use spin_locked_app::MetadataKey;
pub use spin_world::spin::key_value::key_value::Error as V3Error;

/// Metadata key for key-value stores.
pub const KEY_VALUE_STORES_KEY: MetadataKey<Vec<String>> = MetadataKey::new("key_value_stores");
pub use host::{log_cas_error, log_error, Error, KeyValueDispatch, Store, StoreManager};
pub use runtime_config::RuntimeConfig;
use spin_core::async_trait;
pub use util::DelegatingStoreManager;

/// A factor that provides key-value storage.
#[derive(Default)]
pub struct KeyValueFactor {
    _priv: (),
}

impl KeyValueFactor {
    /// Create a new KeyValueFactor.
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl Factor for KeyValueFactor {
    type RuntimeConfig = RuntimeConfig;
    type AppState = AppState;
    type InstanceBuilder = InstanceBuilder;

    fn init(&mut self, ctx: &mut impl InitContext<Self>) -> anyhow::Result<()> {
        ctx.link_bindings(spin_world::v1::key_value::add_to_linker::<_, KvFactorData>)?;
        ctx.link_bindings(spin_world::v2::key_value::add_to_linker::<_, KvFactorData>)?;
        ctx.link_bindings(spin_world::wasi::keyvalue::store::add_to_linker::<_, KvFactorData>)?;
        ctx.link_bindings(spin_world::wasi::keyvalue::batch::add_to_linker::<_, KvFactorData>)?;
        ctx.link_bindings(
            spin_world::wasi::keyvalue::atomics::add_to_linker::<_, KvFactorData>,
        )?;
        ctx.link_bindings(spin_world::spin::key_value::key_value::add_to_linker::<_, KvFactorData>)?;
        Ok(())
    }

    fn configure_app<T: RuntimeFactors>(
        &self,
        mut ctx: ConfigureAppContext<T, Self>,
    ) -> anyhow::Result<Self::AppState> {
        let store_managers = ctx.take_runtime_config().unwrap_or_default();

        let delegating_manager = DelegatingStoreManager::new(store_managers);
        let store_manager = Arc::new(delegating_manager);

        // Build component -> allowed stores map
        let mut component_allowed_stores = HashMap::new();
        for component in ctx.app().components() {
            let component_id = component.id().to_string();
            let key_value_stores = component
                .get_metadata(KEY_VALUE_STORES_KEY)?
                .unwrap_or_default()
                .into_iter()
                .collect::<HashSet<_>>();
            for label in &key_value_stores {
                // TODO: port nicer errors from KeyValueComponent (via error type?)
                ensure!(
                    store_manager.is_defined(label),
                    "unknown key_value_stores label {label:?} for component {component_id:?}"
                );
            }
            component_allowed_stores.insert(component_id, key_value_stores);
            // TODO: warn (?) on unused store?
        }

        Ok(AppState {
            store_manager,
            component_allowed_stores,
        })
    }

    fn prepare<T: RuntimeFactors>(
        &self,
        mut ctx: PrepareContext<T, Self>,
    ) -> anyhow::Result<InstanceBuilder> {
        let app_state = ctx.app_state();
        let allowed_stores = app_state
            .component_allowed_stores
            .get(ctx.app_component().id())
            .expect("component should be in component_stores")
            .clone();
        let otel = OtelFactorState::from_prepare_context(&mut ctx)?;
        Ok(InstanceBuilder {
            store_manager: app_state.store_manager.clone(),
            allowed_stores,
            otel,
        })
    }
}

type AppStoreManager = DelegatingStoreManager;

pub struct AppState {
    /// The store manager for the app.
    ///
    /// This is a cache around a delegating store manager. For `get` requests,
    /// first checks the cache before delegating to the underlying store
    /// manager.
    store_manager: Arc<AppStoreManager>,
    /// The allowed stores for each component.
    ///
    /// This is a map from component ID to the set of store labels that the
    /// component is allowed to use.
    component_allowed_stores: HashMap<String, HashSet<String>>,
}

impl AppState {
    /// Returns the [`StoreManager::summary`] for the given store label.
    pub fn store_summary(&self, label: &str) -> Option<String> {
        self.store_manager.summary(label)
    }

    /// Returns true if the given store label is used by any component.
    pub fn store_is_used(&self, label: &str) -> bool {
        self.component_allowed_stores
            .values()
            .any(|stores| stores.contains(label))
    }

    /// Get a store by label.
    pub async fn get_store(&self, label: &str) -> Option<Arc<dyn Store>> {
        self.store_manager.get(label).await.ok()
    }
}

/// `SwapError` are errors that occur during compare and swap operations
#[derive(Debug, thiserror::Error)]
pub enum SwapError {
    #[error("{0}")]
    CasFailed(String),

    #[error("{0}")]
    Other(String),
}

/// `Cas` trait describes the interface a key value compare and swap implementor must fulfill.
///
/// `current` is expected to get the current value for the key associated with the CAS operation
/// while also starting what is needed to ensure the value to be replaced will not have mutated
/// between the time of calling `current` and `swap`. For example, a get from a backend store
/// may provide the caller with an etag (a version stamp), which can be used with an if-match
/// header to ensure the version updated is the version that was read (optimistic concurrency).
/// Rather than an etag, one could start a transaction, if supported by the backing store, which
/// would provide atomicity.
///
/// `swap` is expected to replace the old value with the new value respecting the atomicity of the
/// operation. If there was no key / value with the given key in the store, the `swap` operation
/// should **insert** the key and value, disallowing an update.
#[async_trait]
pub trait Cas: Sync + Send {
    async fn current(&self) -> anyhow::Result<Option<Vec<u8>>, Error>;
    async fn swap(&self, value: Vec<u8>) -> anyhow::Result<(), SwapError>;
    async fn bucket_rep(&self) -> u32;
    async fn key(&self) -> String;
}

pub struct InstanceBuilder {
    /// The store manager for the app.
    ///
    /// This is a cache around a delegating store manager. For `get` requests,
    /// first checks the cache before delegating to the underlying store
    /// manager.
    store_manager: Arc<AppStoreManager>,
    /// The allowed stores for this component instance.
    allowed_stores: HashSet<String>,
    otel: OtelFactorState,
}

impl FactorInstanceBuilder for InstanceBuilder {
    type InstanceState = KeyValueDispatch;

    fn build(self) -> anyhow::Result<Self::InstanceState> {
        let Self {
            store_manager,
            allowed_stores,
            otel,
        } = self;
        Ok(KeyValueDispatch::new_with_capacity(
            allowed_stores,
            store_manager,
            u32::MAX,
            otel,
        ))
    }
}

pub struct KvFactorData(KeyValueFactor);

impl spin_core::wasmtime::component::HasData for KvFactorData {
    type Data<'a> = &'a mut KeyValueDispatch;
}

impl spin_core::wasmtime::component::HasData for KeyValueDispatch {
    type Data<'a> = &'a mut KeyValueDispatch;
}

use spin_world::spin::key_value::key_value as v3;
use spin_core::wasmtime;

impl v3::HostStoreWithStore for KvFactorData {
    async fn get<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
        key: String,
    ) -> Result<Option<Vec<u8>>, v3::Error> {
        let store = accessor.with(|mut access| {
            let host = access.get();
            host.get_store_fr_fr(self_)
        });
        store.unwrap().get(&key).await.map_err(host::v2_err_to_v3)
    }

    async fn set<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), v3::Error> {
        let store = accessor.with(|mut access| {
            let host = access.get();
            host.get_store_fr_fr(self_)
        });
        store.unwrap().set(&key, &value).await.map_err(host::v2_err_to_v3)
    }

    async fn delete<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
        key: String,
    ) -> Result<(), v3::Error> {
        todo!()
    }

    async fn exists<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
        key: String,
    ) -> Result<bool, v3::Error> {
        todo!()
    }

    async fn set_stream<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
        key: String,
        value: wasmtime::component::StreamReader<u8>,
    ) -> Result<(), v3::Error> {
        todo!()
    }

    async fn get_keys<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
    ) -> wasmtime::Result<(wasmtime::component::StreamReader<String>, wasmtime::component::FutureReader<Result<(),v3::Error>>)> {
        use spin_core::wasmtime::AsContextMut;

        let store = accessor.with(|mut access| {
            let host = access.get();
            host.get_store_fr_fr(self_)
        });
        let (producer, eproducer) = util::wasify(store.unwrap().get_keys_stream().await.unwrap());
        
        let (sr, fr) = accessor.with(|mut access| {
            let sr = wasmtime::component::StreamReader::new(access.instance(), access.as_context_mut(), producer);
            let fr = wasmtime::component::FutureReader::new(access.instance(), access.as_context_mut(), eproducer);
            (sr, fr)
        });

        Ok((sr, fr))
    }
    
    async fn get_stream<T>(
        accessor: &wasmtime::component::Accessor<T,Self>,
        self_: wasmtime::component::Resource<v3::Store>,
        key: String
    ) -> wasmtime::Result<(Option<wasmtime::component::StreamReader<u8>>,wasmtime::component::FutureReader<Result<(),v3::Error>>,)>
    {
        use spin_core::wasmtime::AsContextMut;

        let store = accessor.with(|mut access| {
            let host = access.get();
            host.get_store_fr_fr(self_)
        });

        let (producer, eproducer) = util::wasify_bytes(store.unwrap().get_stream(&key).await.unwrap());

        let (sr, fr) = accessor.with(|mut access| {
            let sr = wasmtime::component::StreamReader::new(access.instance(), access.as_context_mut(), producer);
            let fr = wasmtime::component::FutureReader::new(access.instance(), access.as_context_mut(), eproducer);
            (sr, fr)
        });

        // okay this makes mock of the Option. We presumably need another way to signal "key not found"
        // or maybe that is the error thing
        Ok((Some(sr), fr))
    }
}
