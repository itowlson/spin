mod host;
pub mod runtime_config;
mod util;

pub mod cackle_cackle;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::ensure;
use spin_factors::{
    ConfigureAppContext, Factor, InitContext, PrepareContext, RuntimeFactors,
};
use spin_locked_app::MetadataKey;
use spin_resource_table::Table;

/// Metadata key for key-value stores.
pub const BLOB_STORES_KEY: MetadataKey<Vec<String>> = MetadataKey::new("blob_containers");
pub use spin_world::wasi::blobstore::types::{ContainerMetadata, ObjectMetadata};
pub use host::{log_error, Error, BlobStoreDispatch, Container, ContainerManager, IncomingData, ObjectNames, OutgoingValue, Finishable};
pub use runtime_config::RuntimeConfig;
use tokio::sync::RwLock;
pub use util::DelegatingContainerManager;
pub use cackle_cackle::write_stream::AsyncWriteStream;

/// A factor that provides key-value storage.
#[derive(Default)]
pub struct BlobStoreFactor {
    _priv: (),
}

impl BlobStoreFactor {
    /// Create a new KeyValueFactor.
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl Factor for BlobStoreFactor {
    type RuntimeConfig = RuntimeConfig;
    type AppState = AppState;
    type InstanceBuilder = InstanceBuilder;

    fn init<T: Send + 'static>(&mut self, mut ctx: InitContext<T, Self>) -> anyhow::Result<()> {
        fn type_annotate<T, F>(f: F) -> F
        where
            F: Fn(&mut T) -> BlobStoreDispatch,
        {
            f
        }

        let get_data_with_table = ctx.get_data_with_table_fn();
        let closure = type_annotate(move |data| {
            let (state, table) = get_data_with_table(data);
            let wasi = wasmtime_wasi::WasiImpl(host::WasiImplInner { ctx: &mut state.ctx, table });
            BlobStoreDispatch::new(
                state.allowed_stores.clone(),
                state.store_manager.clone(),
                wasi,
                state.containers.clone(),
                state.incoming_values.clone(),
                state.outgoing_values.clone(),
                state.object_names.clone(),
            )
        });
        let linker = ctx.linker();

        spin_world::wasi::blobstore::blobstore::add_to_linker_get_host(linker, closure)?;
        spin_world::wasi::blobstore::container::add_to_linker_get_host(linker, closure)?;
        spin_world::wasi::blobstore::types::add_to_linker_get_host(linker, closure)?;

        Ok(())
    }

    fn configure_app<T: RuntimeFactors>(
        &self,
        mut ctx: ConfigureAppContext<T, Self>,
    ) -> anyhow::Result<Self::AppState> {
        let store_managers = ctx.take_runtime_config().unwrap_or_default();

        let delegating_manager = DelegatingContainerManager::new(store_managers);
        // let caching_manager = CachingStoreManager::new(delegating_manager);
        let store_manager = Arc::new(delegating_manager);

        // Build component -> allowed stores map
        let mut component_allowed_stores = HashMap::new();
        for component in ctx.app().components() {
            let component_id = component.id().to_string();
            let containers = component
                .get_metadata(BLOB_STORES_KEY)?
                .unwrap_or_default()
                .into_iter()
                .collect::<HashSet<_>>();
            for label in &containers {
                // TODO: port nicer errors from KeyValueComponent (via error type?)
                ensure!(
                    store_manager.is_defined(label),
                    "unknown key_value_stores label {label:?} for component {component_id:?}"
                );
            }
            component_allowed_stores.insert(component_id, containers);
            // TODO: warn (?) on unused store?
        }

        Ok(AppState {
            container_manager: store_manager,
            component_allowed_stores,
        })
    }

    fn prepare<T: RuntimeFactors>(
        &self,
        ctx: PrepareContext<T, Self>,
    ) -> anyhow::Result<InstanceBuilder> {
        let mut wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new();

        let app_state = ctx.app_state();
        let allowed_stores = app_state
            .component_allowed_stores
            .get(ctx.app_component().id())
            .expect("component should be in component_stores")
            .clone();
        let capacity = u32::MAX;
        Ok(InstanceBuilder {
            store_manager: app_state.container_manager.clone(),
            allowed_stores,
            ctx: wasi_ctx.build(),
            containers: Arc::new(RwLock::new(Table::new(capacity))),
            incoming_values: Arc::new(RwLock::new(Table::new(capacity))),
            object_names: Arc::new(RwLock::new(Table::new(capacity))),
            outgoing_values: Arc::new(RwLock::new(Table::new(capacity))),
        })
    }
}

type AppStoreManager = DelegatingContainerManager;

pub struct AppState {
    /// The store manager for the app.
    ///
    /// This is a cache around a delegating store manager. For `get` requests,
    /// first checks the cache before delegating to the underlying store
    /// manager.
    container_manager: Arc<AppStoreManager>,
    /// The allowed stores for each component.
    ///
    /// This is a map from component ID to the set of store labels that the
    /// component is allowed to use.
    component_allowed_stores: HashMap<String, HashSet<String>>,
}

impl AppState {
    /// Returns the [`StoreManager::summary`] for the given store label.
    pub fn store_summary(&self, label: &str) -> Option<String> {
        self.container_manager.summary(label)
    }

    /// Returns true if the given store label is used by any component.
    pub fn store_is_used(&self, label: &str) -> bool {
        self.component_allowed_stores
            .values()
            .any(|stores| stores.contains(label))
    }

    /// Get a store by label.
    pub async fn get_container(&self, label: &str) -> Option<Arc<dyn Container>> {
        self.container_manager.get(label).await.ok()
    }
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
    ctx: wasmtime_wasi::WasiCtx,
    containers: Arc<RwLock<Table<Arc<dyn Container>>>>,
    incoming_values: Arc<RwLock<Table<Box<dyn IncomingData>>>>,
    outgoing_values: Arc<RwLock<Table<host::OutgoingValue>>>,
    object_names: Arc<RwLock<Table<Box<dyn ObjectNames>>>>,
}

impl spin_factors::SelfInstanceBuilder for InstanceBuilder {}