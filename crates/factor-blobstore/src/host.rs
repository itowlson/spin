use anyhow::{Context, Result};
use spin_core::{async_trait, wasmtime::component::Resource};
use spin_resource_table::Table;
use spin_world::wasi::blobstore;
use std::{collections::HashSet, sync::Arc};
// use tracing::{instrument, Level};

const DEFAULT_STORE_TABLE_CAPACITY: u32 = 256;

pub use blobstore::types::Error;

#[async_trait]
pub trait ContainerManager: Sync + Send {
    async fn get(&self, name: &str) -> Result<Arc<dyn Container>, Error>;
    fn is_defined(&self, container_name: &str) -> bool;

    /// A human-readable summary of the given container's configuration
    ///
    /// Example: "Azure blob container 'biscuit-tin'"
    fn summary(&self, store_name: &str) -> Option<String> {
        let _ = store_name;
        None
    }
}

#[async_trait]
pub trait Container: Sync + Send {
    async fn exists(&self) -> anyhow::Result<bool>;
    async fn name(&self) -> String;
    async fn info(&self) -> anyhow::Result<blobstore::types::ContainerMetadata>;
    async fn clear(&self) -> anyhow::Result<()>;
    async fn delete_object(&self, name: &str) -> anyhow::Result<()>;
    async fn delete_objects(&self, names: &[String]) -> anyhow::Result<()>;
    async fn has_object(&self, name: &str) -> anyhow::Result<bool>;
    async fn object_info(&self, name: &str) -> anyhow::Result<blobstore::types::ObjectMetadata>;
    async fn get_data(&self, name: &str, start: u64, end: u64) -> anyhow::Result<Box<dyn IncomingData>>;
    async fn list_objects(&self) -> anyhow::Result<Box<dyn ObjectNames>>;
}

#[async_trait]
pub trait ObjectNames : Send + Sync {
    async fn read(&mut self, len: u64) -> anyhow::Result<(Vec<String>, bool)>;
    async fn skip(&mut self, num: u64) -> anyhow::Result<(u64,bool)>;
}

#[async_trait]
pub trait IncomingData : Send + Sync {
    async fn consume_sync(&mut self) -> anyhow::Result<Vec<u8>>;
    async fn size(&mut self) -> anyhow::Result<u64>;
}

pub struct BlobStoreDispatch {
    allowed_containers: HashSet<String>,
    manager: Arc<dyn ContainerManager>,
    containers: Table<Arc<dyn Container>>,
    incoming_values: Table<Box<dyn IncomingData>>,
    object_names: Table<Box<dyn ObjectNames>>,
}

impl BlobStoreDispatch {
    pub fn new(allowed_containers: HashSet<String>, manager: Arc<dyn ContainerManager>) -> Self {
        Self::new_with_capacity(allowed_containers, manager, DEFAULT_STORE_TABLE_CAPACITY)
    }

    pub fn new_with_capacity(
        allowed_container: HashSet<String>,
        manager: Arc<dyn ContainerManager>,
        capacity: u32,
    ) -> Self {
        Self {
            allowed_containers: allowed_container,
            manager,
            containers: Table::new(capacity),
            incoming_values: Table::new(capacity),
            object_names: Table::new(capacity),
        }
    }

    pub fn get_container(&self, container: Resource<blobstore::blobstore::Container>) -> anyhow::Result<&Arc<dyn Container>> {
        self.containers.get(container.rep()).context("invalid container")
    }

    pub fn allowed_containers(&self) -> &HashSet<String> {
        &self.allowed_containers
    }

    fn container(&self, resource: Resource<blobstore::container::Container>) -> Result<&Arc<dyn Container>, String> {
        self.containers.get(resource.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )
    }

    fn object_names(&mut self, resource: Resource<blobstore::container::StreamObjectNames>) -> Result<&mut Box<dyn ObjectNames>, String> {
        self.object_names.get_mut(resource.rep()).ok_or_else(||
            "invalid stream-object-names resource".to_string()
        )
    }

    fn incoming_value(&mut self, resource: Resource<blobstore::container::IncomingValue>) -> Result<&mut Box<dyn IncomingData>, String> {
        self.incoming_values.get_mut(resource.rep()).ok_or_else(||
            "invalid incoming-value resource".to_string()
        )
    }

    fn take_incoming_value(&mut self, resource: Resource<blobstore::container::IncomingValue>) -> Result<Box<dyn IncomingData>, String> {
        self.incoming_values.remove(resource.rep()).ok_or_else(||
            "invalid incoming-value resource".to_string()
        )
    }
}

#[async_trait]
impl blobstore::blobstore::Host for BlobStoreDispatch {
    async fn create_container(&mut self, _name: String) -> Result<Resource<blobstore::container::Container>, String> {
        Err("This version of Spin does not support creating containers".to_owned())
    }

    async fn get_container(&mut self, name: String) -> Result<Resource<blobstore::container::Container>, String> {
        if self.allowed_containers.contains(&name) {
            let container = self.manager.get(&name).await?;
            let rep = self.containers.push(container).unwrap();
            Ok(Resource::new_own(rep))
        } else {
            Err("forbidden container".to_owned())
        }
    }

    async fn delete_container(&mut self, _name: String) -> Result<(), String> {
        Err("This version of Spin does not support deleting containers".to_owned())
    }

    async fn container_exists(&mut self, name: String) -> Result<bool, String> {
        if self.allowed_containers.contains(&name) {
            let container = self.manager.get(&name).await?;
            container.exists().await.map_err(|e| e.to_string())
        } else {
            Ok(false)
        }
    }

    async fn copy_object(&mut self, _src: blobstore::blobstore::ObjectId, _dest: blobstore::blobstore::ObjectId) -> Result<(), String> {
        Err("This version of Spin does not support copying objects".to_owned())
    }

    async fn move_object(&mut self, _src: blobstore::blobstore::ObjectId, _dest: blobstore::blobstore::ObjectId) -> Result<(), String> {
        Err("This version of Spin does not support moving objects".to_owned())
    }
}

#[async_trait]
impl blobstore::types::Host for BlobStoreDispatch {
    fn convert_error(&mut self, error: String) -> anyhow::Result<String> {
        Ok(error)
    }
}

#[async_trait]
impl blobstore::types::HostIncomingValue for BlobStoreDispatch {
    async fn incoming_value_consume_sync(&mut self, self_: Resource<blobstore::types::IncomingValue>) -> Result<Vec<u8>, String> {
        let mut incoming = self.take_incoming_value(self_)?;
        incoming.as_mut().consume_sync().await.map_err(|e| e.to_string())
    }

    async fn incoming_value_consume_async(&mut self, self_: Resource<blobstore::types::IncomingValue>) -> Result<Resource<blobstore::types::InputStream>, String> {
        todo!()
    }

    async fn size(&mut self, self_: Resource<blobstore::types::IncomingValue>) -> anyhow::Result<u64> {
        let incoming = self.incoming_value(self_).map_err(|s| anyhow::anyhow!("{s}"))?;
        incoming.size().await
    }

    async fn drop(&mut self, rep: Resource<blobstore::types::IncomingValue>) -> anyhow::Result<()> {
        self.incoming_values.remove(rep.rep());
        Ok(())
    }
}

#[async_trait]
impl blobstore::types::HostOutgoingValue for BlobStoreDispatch {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<blobstore::types::OutgoingValue>> {
        todo!()
    }

    async fn outgoing_value_write_body(&mut self, self_: Resource<blobstore::types::OutgoingValue>) -> anyhow::Result<Result<Resource<blobstore::types::OutputStream>, ()>> {
        todo!()
    }

    async fn finish(&mut self, self_: Resource<blobstore::types::OutgoingValue>) -> Result<(), String> {
        todo!()
    }

    async fn drop(&mut self, rep: Resource<blobstore::types::OutgoingValue>) -> anyhow::Result<()> {
        todo!()
    }
}

// TODO: TBD if these belong on BSD or some other struct (like the one that maps to a Container resource JUST SAYIN)
#[async_trait]
impl blobstore::container::Host for BlobStoreDispatch {}

#[async_trait]
impl blobstore::container::HostContainer for BlobStoreDispatch {
    async fn name(&mut self, self_: Resource<blobstore::container::Container>) -> Result<String, String> {
        let container = self.container(self_)?;
        Ok(container.name().await)
    }

    async fn info(&mut self, self_: Resource<blobstore::container::Container>) -> Result<blobstore::container::ContainerMetadata, String> {
        let container = self.container(self_)?;
        container.info().await.map_err(|e| e.to_string())
    }

    async fn get_data(&mut self, self_: Resource<blobstore::container::Container>, name: blobstore::container::ObjectName, start: u64, end: u64) -> Result<Resource<blobstore::types::IncomingValue>, String> {
        let container = self.container(self_)?;
        let incoming = container.get_data(&name, start, end).await.map_err(|e| e.to_string())?;
        let rep = self.incoming_values.push(incoming).unwrap();
        Ok(Resource::new_own(rep))
    }

    async fn write_data(&mut self, self_: Resource<blobstore::container::Container>, name: blobstore::container::ObjectName, data: Resource<blobstore::types::OutgoingValue>) -> Result<(), String> {
        todo!()
    }

    async fn list_objects(&mut self, self_: Resource<blobstore::container::Container>) -> Result<Resource<blobstore::container::StreamObjectNames>, String> {
        let container = self.container(self_)?;
        let names = container.list_objects().await.map_err(|e| e.to_string())?;
        let rep = self.object_names.push(names).unwrap();
        Ok(Resource::new_own(rep))
    }

    async fn delete_object(&mut self, self_: Resource<blobstore::container::Container>, name: String) -> Result<(), String> {
        let container = self.container(self_)?;
        container.delete_object(&name).await.map_err(|e| e.to_string())
    }

    async fn delete_objects(&mut self, self_: Resource<blobstore::container::Container>, names: Vec<String>) -> Result<(), String> {
        let container = self.container(self_)?;
        container.delete_objects(&names).await.map_err(|e| e.to_string())
    }

    async fn has_object(&mut self, self_: Resource<blobstore::container::Container>, name: String) -> Result<bool, String> {
        let container = self.container(self_)?;
        container.has_object(&name).await.map_err(|e| e.to_string())
    }

    async fn object_info(&mut self, self_: Resource<blobstore::container::Container>, name: String) -> Result<blobstore::types::ObjectMetadata, String> {
        let container = self.container(self_)?;
        container.object_info(&name).await.map_err(|e| e.to_string())
    }

    async fn clear(&mut self, self_: Resource<blobstore::container::Container>) -> Result<(), String> {
        let container = self.container(self_)?;
        container.clear().await.map_err(|e| e.to_string())
    }

    async fn drop(&mut self, rep: Resource<blobstore::container::Container>) -> anyhow::Result<()> {
        self.containers.remove(rep.rep());
        Ok(())
    }
}

#[async_trait]
impl blobstore::container::HostStreamObjectNames for BlobStoreDispatch {
    async fn read_stream_object_names(&mut self, self_: Resource<blobstore::container::StreamObjectNames>, len: u64) -> Result<(Vec<String>,bool), String> {
        let object_names = self.object_names(self_)?;
        object_names.as_mut().read(len).await.map_err(|e| e.to_string())
    }

    async fn skip_stream_object_names(&mut self, self_: Resource<blobstore::container::StreamObjectNames>, num: u64) -> Result<(u64,bool), String> {
        let object_names = self.object_names(self_)?;
        object_names.as_mut().skip(num).await.map_err(|e| e.to_string())
    }

    async fn drop(&mut self, rep: Resource<blobstore::container::StreamObjectNames>) -> anyhow::Result<()> {
        self.object_names.remove(rep.rep());
        Ok(())
    }
}

pub fn log_error(err: impl std::fmt::Debug) -> String {
    tracing::warn!("blobstore error: {err:?}");
    format!("{err:?}")
}
