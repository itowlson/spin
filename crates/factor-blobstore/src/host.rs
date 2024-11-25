use anyhow::{Context, Result};
use spin_core::{async_trait, wasmtime::component::Resource};
use spin_resource_table::Table;
use spin_world::wasi::blobstore;
use tokio::sync::RwLock;
use std::{collections::HashSet, sync::Arc};
// use tracing::{instrument, Level};
use wasmtime_wasi::WasiView;

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
    async fn attach_writer(&self, name: &str, data: &OutgoingValue) -> anyhow::Result<()>;
    // async fn get_write_stream(&self, name: &str) -> anyhow::Result<(/*wasmtime_wasi::pipe*/ crate::AsyncWriteStream, Box<dyn Finishable>)>;
    async fn connect_stm(&self, name: &str, stm: tokio::io::ReadHalf<tokio::io::SimplexStream>) -> anyhow::Result<()>;
    // async fn write_data(&self, name: &str, data: spin_core::wasmtime::component::Resource<blobstore::types::OutgoingValue>) -> anyhow::Result<()>;
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
    fn consume_async(&mut self) -> wasmtime_wasi::pipe::AsyncReadStream;
    async fn size(&mut self) -> anyhow::Result<u64>;
}

pub struct OutgoingValue {
    // I wonder if this is more a pipe - then write_async gets you the
    // write end of the pipe, and container::write-data hooks up the read
    // end of the pipe to the back-end store?
    // stm: Option<wasmtime_wasi::pipe::AsyncWriteStream>,
    // finish: Option<Box<dyn Finishable>>,
    read: Option<tokio::io::ReadHalf<tokio::io::SimplexStream>>,
    write: Option<tokio::io::WriteHalf<tokio::io::SimplexStream>>,
    write_rep: Option<u32>,
    stop_tx: Option<tokio::sync::mpsc::Sender<()>>
}

const OUTGOING_VALUE_BUF_SIZE: usize = 16 * 1024;

impl OutgoingValue {
    fn new() -> Self {
        let (read, write) = tokio::io::simplex(OUTGOING_VALUE_BUF_SIZE);
        Self {
            read: Some(read),
            write: Some(write),
            write_rep: None,
            stop_tx: None,
       }
    }

    fn write_stream(&mut self) -> anyhow::Result<crate::AsyncWriteStream> {
        let Some(write) = self.write.take() else {
            anyhow::bail!("OutgoingValue has already returned its write stream");
        };

        let (stop_tx, stop_rx) = tokio::sync::mpsc::channel::<()>(1);

        self.stop_tx = Some(stop_tx);

        let stm = /*wasmtime_wasi::pipe*/crate::AsyncWriteStream::new(OUTGOING_VALUE_BUF_SIZE, write, stop_rx);
        Ok(stm)
    }

    fn set_write_rep(&mut self, rep: u32) {
        self.write_rep = Some(rep);
    }

    fn write_rep(&self) -> Option<u32> {
        self.write_rep
    }

    fn stop_tx(&self) -> Option<&tokio::sync::mpsc::Sender<()>> {
        self.stop_tx.as_ref()
    }

    fn take_read_stream(&mut self) -> anyhow::Result<tokio::io::ReadHalf<tokio::io::SimplexStream>> {
        let Some(read) = self.read.take() else {
            anyhow::bail!("OutgoingValue has already been connected to a blob");
        };

        // let stm = wasmtime_wasi::pipe::AsyncReadStream::new(read);
        // Ok(stm)
        Ok(read)
    }

    async fn finish(&mut self) -> anyhow::Result<()> {
        todo!()
    }
}

// struct Spork<T>(Arc<std::sync::RwLock<T>>);

// impl<T: tokio::io::AsyncWrite> tokio::io::AsyncWrite for Spork<T> {
//     fn poll_write(
//         self: std::pin::Pin<&mut Self>,
//         cx: &mut std::task::Context<'_>,
//         buf: &[u8],
//     ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
//         use std::ops::DerefMut;
//         let mut lock = self.0.write().unwrap();
//         let r: &mut T = lock.deref_mut();
//         let p = std::pin::pin!(r);
//         p.poll_write(cx, buf)
//     }

//     fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
//         todo!()
//     }

//     fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
//         todo!()
//     }
// }

#[async_trait]
pub trait Finishable : Send + Sync {
    async fn finish(&mut self);
}

pub struct BlobStoreDispatch<'a> {
    allowed_containers: HashSet<String>,
    manager: Arc<dyn ContainerManager>,
    wasi: wasmtime_wasi::WasiImpl<WasiImplInner<'a>>,
    containers: Arc<RwLock<Table<Arc<dyn Container>>>>,
    incoming_values: Arc<RwLock<Table<Box<dyn IncomingData>>>>,
    //outgoing_values: Arc<RwLock<Table<OutgoingValue>>>,
    object_names: Arc<RwLock<Table<Box<dyn ObjectNames>>>>,
}

pub struct WasiImplInner<'a> {
    pub ctx: &'a mut wasmtime_wasi::WasiCtx,
    pub table: &'a mut spin_core::wasmtime::component::ResourceTable,
}

impl<'a> wasmtime_wasi::WasiView for WasiImplInner<'a> {
    fn ctx(&mut self) -> &mut wasmtime_wasi::WasiCtx {
        self.ctx
    }

    fn table(&mut self) -> &mut spin_core::wasmtime::component::ResourceTable {
        self.table
    }
}

impl<'a> BlobStoreDispatch<'a> {
    pub(crate) fn new(allowed_containers: HashSet<String>,
        manager: Arc<dyn ContainerManager>,
        wasi: wasmtime_wasi::WasiImpl<WasiImplInner<'a>>,
        containers: Arc<RwLock<Table<Arc<dyn Container>>>>,
        incoming_values: Arc<RwLock<Table<Box<dyn IncomingData>>>>,
        //outgoing_values: Arc<RwLock<Table<OutgoingValue>>>,
        object_names: Arc<RwLock<Table<Box<dyn ObjectNames>>>>,
    ) -> Self {
        Self {
            allowed_containers,
            manager,
            wasi,
            containers,
            incoming_values,
            //outgoing_values,
            object_names,
        }
    }

    pub async fn get_container(&self, container: Resource<blobstore::blobstore::Container>) -> anyhow::Result<Arc<dyn Container>> {
        self.containers.read().await.get(container.rep()).context("invalid container").cloned()
    }

    pub fn allowed_containers(&self) -> &HashSet<String> {
        &self.allowed_containers
    }

    async fn take_incoming_value(&mut self, resource: Resource<blobstore::container::IncomingValue>) -> Result<Box<dyn IncomingData>, String> {
        self.incoming_values.write().await.remove(resource.rep()).ok_or_else(||
            "invalid incoming-value resource".to_string()
        )
    }
}

#[async_trait]
impl<'a> blobstore::blobstore::Host for BlobStoreDispatch<'a> {
    async fn create_container(&mut self, _name: String) -> Result<Resource<blobstore::container::Container>, String> {
        Err("This version of Spin does not support creating containers".to_owned())
    }

    async fn get_container(&mut self, name: String) -> Result<Resource<blobstore::container::Container>, String> {
        if self.allowed_containers.contains(&name) {
            let container = self.manager.get(&name).await?;
            let rep = self.containers.write().await.push(container).unwrap();
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
impl<'a> blobstore::types::Host for BlobStoreDispatch<'a> {
    fn convert_error(&mut self, error: String) -> anyhow::Result<String> {
        Ok(error)
    }
}

#[async_trait]
impl<'a> blobstore::types::HostIncomingValue for BlobStoreDispatch<'a> {
    async fn incoming_value_consume_sync(&mut self, self_: Resource<blobstore::types::IncomingValue>) -> Result<Vec<u8>, String> {
        let mut incoming = self.take_incoming_value(self_).await?;
        incoming.as_mut().consume_sync().await.map_err(|e| e.to_string())
    }

    async fn incoming_value_consume_async(&mut self, self_: Resource<blobstore::types::IncomingValue>) -> Result<Resource<wasmtime_wasi::InputStream>, String> {
        let mut incoming = self.take_incoming_value(self_).await?;
        let async_body = incoming.as_mut().consume_async();
        let host_stm: Box<dyn wasmtime_wasi::HostInputStream> = Box::new(async_body);
        let resource = self.wasi.table().push(host_stm).unwrap();
        Ok(resource)
    }

    async fn size(&mut self, self_: Resource<blobstore::types::IncomingValue>) -> anyhow::Result<u64> {
        let mut lock = self.incoming_values.write().await;
        let incoming = lock.get_mut(self_.rep()).ok_or_else(|| anyhow::anyhow!("invalid incoming-value resource"))?;
        incoming.size().await
    }

    async fn drop(&mut self, rep: Resource<blobstore::types::IncomingValue>) -> anyhow::Result<()> {
        self.incoming_values.write().await.remove(rep.rep());
        Ok(())
    }
}

#[async_trait]
impl<'a> blobstore::types::HostOutgoingValue for BlobStoreDispatch<'a> {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<blobstore::types::OutgoingValue>> {
        let outgoing_value = OutgoingValue::new(); // OutgoingValue { stm: None, finish: None };
        let ov_wrapped: spin_world::BoxedAny = Box::new(outgoing_value);
        // let rep = self.outgoing_values.write().await.push(outgoing_value).unwrap();
        // Ok(Resource::new_own(rep))
        let resource = self.wasi.table().push(ov_wrapped).unwrap();
        Ok(resource)
    }

    async fn outgoing_value_write_body(&mut self, self_: Resource<blobstore::types::OutgoingValue>) -> anyhow::Result<Result<Resource<wasmtime_wasi::OutputStream>, ()>> {
        // let mut lock = self.outgoing_values.write().await;
        // let outgoing = lock.get_mut(self_.rep()).ok_or_else(|| anyhow::anyhow!("invalid outgoing-value resource"))?;
        let stm = {
        let anyed_ov = self.wasi.table().get_mut(&self_).unwrap();
        let outgoing = anyed_ov.downcast_mut::<OutgoingValue>().expect("shoulda downcast");
        let stm = outgoing.write_stream()?;
        stm
        };

        let resource = {
        let host_stm: Box<dyn wasmtime_wasi::HostOutputStream> = Box::new(stm);
        let resource = self.wasi.table().push(host_stm).unwrap();
        // let resource = self.wasi.table().push_child(host_stm, &self_).unwrap();
        resource
        };

        let anyed_ov = self.wasi.table().get_mut(&self_).unwrap();
        let outgoing = anyed_ov.downcast_mut::<OutgoingValue>().expect("shoulda downcast");
        outgoing.set_write_rep(resource.rep());
        Ok(Ok(resource))
    }

    async fn finish(&mut self, self_: Resource<blobstore::types::OutgoingValue>) -> Result<(), String> {
        let stop_tx = {
            let anyed_ov = self.wasi.table().get_mut(&self_).unwrap();
            let outgoing = anyed_ov.downcast_mut::<OutgoingValue>().expect("shoulda downcast");
            outgoing.stop_tx().expect("shoulda had a stop_tx")
        };

        // let mut lock = self.outgoing_values.write().await;
        // let outgoing = lock.get_mut(self_.rep()).ok_or_else(|| "invalid outgoing-value resource".to_owned())?;
        // let write_rep = outgoing.write_rep().ok_or_else(|| "no stm".to_string())?;
        // let any = self.wasi.table().get_any_mut(write_rep).expect("we didn't get the Any");
        // println!("we got a {}", std::any::type_name_of_val(any));
        // let stm: &mut Box<dyn wasmtime_wasi::HostOutputStream> = any.downcast_mut().expect("we didn't get the stm");
        // // use tokio::io::AsyncWriteExt;
        // // use futures::
        // println!("well we kinda got it");
        // stm.flush().unwrap();
        // outgoing.finish().await.map_err(|e| e.to_string())?;

        // self.wasi.table().delete(self_).unwrap();
        // Ok(())

        // let stm_any = self.wasi.table().get_any_mut(write_rep).expect("shoulda had a resource for stm");

        // // let mut ch = self.wasi.table().iter_children(&self_).expect("children");
        // // let stm_ch = ch.next().expect("shoulda had a child");
        // let stm = stm_any.downcast_mut::<Box<dyn wasmtime_wasi::HostOutputStream>>().expect("downcast to dyn stm");
        // // let stm = stm.as
        // stm.shutdown().await.expect("shoulda shut down");
        stop_tx.send(()).await.expect("shoulda sent a stop");
        println!("****** SHUTDOWN SENT");
        Ok(())
        // drop(stm);
        // Ok(())
    }

    async fn drop(&mut self, rep: Resource<blobstore::types::OutgoingValue>) -> anyhow::Result<()> {
        // self.outgoing_values.write().await.remove(rep.rep());
        self.wasi.table().delete(rep)?;
        Ok(())
    }
}

// TODO: TBD if these belong on BSD or some other struct (like the one that maps to a Container resource JUST SAYIN)
#[async_trait]
impl<'a> blobstore::container::Host for BlobStoreDispatch<'a> {}

#[async_trait]
impl<'a> blobstore::container::HostContainer for BlobStoreDispatch<'a> {
    async fn name(&mut self, self_: Resource<blobstore::container::Container>) -> Result<String, String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        Ok(container.name().await)
    }

    async fn info(&mut self, self_: Resource<blobstore::container::Container>) -> Result<blobstore::container::ContainerMetadata, String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        container.info().await.map_err(|e| e.to_string())
    }

    async fn get_data(&mut self, self_: Resource<blobstore::container::Container>, name: blobstore::container::ObjectName, start: u64, end: u64) -> Result<Resource<blobstore::types::IncomingValue>, String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        let incoming = container.get_data(&name, start, end).await.map_err(|e| e.to_string())?;
        let rep = self.incoming_values.write().await.push(incoming).unwrap();
        Ok(Resource::new_own(rep))
    }

    async fn write_data(&mut self, self_: Resource<blobstore::container::Container>, name: blobstore::container::ObjectName, data: Resource<blobstore::types::OutgoingValue>) -> Result<(), String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        // let mut lock2 = self.outgoing_values.write().await;
        // let outgoing = lock2.get_mut(data.rep()).ok_or_else(||
        //     "invalid outgoing-value resource".to_string()
        // )?;
        let anyed_ov = self.wasi.table().get_mut(&data).unwrap();
        let outgoing = anyed_ov.downcast_mut::<OutgoingValue>().expect("shoulda downcast");

        let stm = outgoing.take_read_stream().map_err(|e| e.to_string())?;
        container.connect_stm(&name, stm).await.map_err(|e| e.to_string())?;

        // let (stm, finish) = container.get_write_stream(&name).await.map_err(|e| e.to_string())?;
        // outgoing.stm = Some(stm);
        // outgoing.finish = Some(finish);
        Ok(())
    }

    async fn list_objects(&mut self, self_: Resource<blobstore::container::Container>) -> Result<Resource<blobstore::container::StreamObjectNames>, String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        let names = container.list_objects().await.map_err(|e| e.to_string())?;
        let rep = self.object_names.write().await.push(names).unwrap();
        Ok(Resource::new_own(rep))
    }

    async fn delete_object(&mut self, self_: Resource<blobstore::container::Container>, name: String) -> Result<(), String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        container.delete_object(&name).await.map_err(|e| e.to_string())
    }

    async fn delete_objects(&mut self, self_: Resource<blobstore::container::Container>, names: Vec<String>) -> Result<(), String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        container.delete_objects(&names).await.map_err(|e| e.to_string())
    }

    async fn has_object(&mut self, self_: Resource<blobstore::container::Container>, name: String) -> Result<bool, String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        container.has_object(&name).await.map_err(|e| e.to_string())
    }

    async fn object_info(&mut self, self_: Resource<blobstore::container::Container>, name: String) -> Result<blobstore::types::ObjectMetadata, String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        container.object_info(&name).await.map_err(|e| e.to_string())
    }

    async fn clear(&mut self, self_: Resource<blobstore::container::Container>) -> Result<(), String> {
        let lock = self.containers.read().await;
        let container = lock.get(self_.rep()).ok_or_else(||
            "invalid container resource".to_string()
        )?;
        container.clear().await.map_err(|e| e.to_string())
    }

    async fn drop(&mut self, rep: Resource<blobstore::container::Container>) -> anyhow::Result<()> {
        self.containers.write().await.remove(rep.rep());
        Ok(())
    }
}

#[async_trait]
impl<'a> blobstore::container::HostStreamObjectNames for BlobStoreDispatch<'a> {
    async fn read_stream_object_names(&mut self, self_: Resource<blobstore::container::StreamObjectNames>, len: u64) -> Result<(Vec<String>,bool), String> {
        let mut lock = self.object_names.write().await;
        let object_names = lock.get_mut(self_.rep()).ok_or_else(||
            "invalid stream-object-names resource".to_string()
        )?;
        object_names.read(len).await.map_err(|e| e.to_string())
    }

    async fn skip_stream_object_names(&mut self, self_: Resource<blobstore::container::StreamObjectNames>, num: u64) -> Result<(u64,bool), String> {
        let mut lock = self.object_names.write().await;
        let object_names = lock.get_mut(self_.rep()).ok_or_else(||
            "invalid stream-object-names resource".to_string()
        )?;
        object_names.skip(num).await.map_err(|e| e.to_string())
    }

    async fn drop(&mut self, rep: Resource<blobstore::container::StreamObjectNames>) -> anyhow::Result<()> {
        self.object_names.write().await.remove(rep.rep());
        Ok(())
    }
}

pub fn log_error(err: impl std::fmt::Debug) -> String {
    tracing::warn!("blobstore error: {err:?}");
    format!("{err:?}")
}
