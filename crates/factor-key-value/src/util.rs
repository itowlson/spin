use crate::{Error, Store, StoreManager};
use spin_core::async_trait;
use std::{collections::HashMap, sync::Arc};

/// A [`StoreManager`] which delegates to other `StoreManager`s based on the store label.
pub struct DelegatingStoreManager {
    delegates: HashMap<String, Arc<dyn StoreManager>>,
}

impl DelegatingStoreManager {
    pub fn new(delegates: impl IntoIterator<Item = (String, Arc<dyn StoreManager>)>) -> Self {
        let delegates = delegates.into_iter().collect();
        Self { delegates }
    }
}

#[async_trait]
impl StoreManager for DelegatingStoreManager {
    async fn get(&self, name: &str) -> Result<Arc<dyn Store>, Error> {
        match self.delegates.get(name) {
            Some(store) => store.get(name).await,
            None => Err(Error::NoSuchStore),
        }
    }

    fn is_defined(&self, store_name: &str) -> bool {
        self.delegates.contains_key(store_name)
    }

    fn summary(&self, store_name: &str) -> Option<String> {
        if let Some(store) = self.delegates.get(store_name) {
            return store.summary(store_name);
        }
        None
    }
}

use spin_core::wasmtime;

pub(crate) fn wasify<T, E>((rx, erx): (tokio::sync::mpsc::Receiver<T>, tokio::sync::oneshot::Receiver<E>)) -> (RxStreamProducer<T>, RxFutureProducer<E>) {
    (RxStreamProducer { rx }, RxFutureProducer { rx: erx })
}

pub(crate) fn wasify_bytes<E>((rx, erx): (tokio::sync::mpsc::Receiver<bytes::Bytes>, tokio::sync::oneshot::Receiver<E>)) -> (ChunkfulRxStreamProducer, RxFutureProducer<E>) {
    (ChunkfulRxStreamProducer { rx }, RxFutureProducer { rx: erx })
}

pub(crate) struct RxStreamProducer<T> {
    rx: tokio::sync::mpsc::Receiver<T>,
}

impl<T: Send + Sync + 'static, D> wasmtime::component::StreamProducer<D> for RxStreamProducer<T> {
    type Item = T;

    type Buffer = Option<Self::Item>;

    fn poll_produce<'a>(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        store: wasmtime::StoreContextMut<'a, D>,
        mut destination: wasmtime::component::Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> std::task::Poll<anyhow::Result<wasmtime::component::StreamResult>> {
        use std::task::Poll;
        use wasmtime::component::StreamResult;

        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        let remaining = destination.remaining(store);
        if remaining.is_some_and(|r| r == 0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let recv = self.get_mut().rx.poll_recv(cx);
        match recv {
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(row)) => {
                destination.set_buffer(Some(row));
                Poll::Ready(Ok(StreamResult::Completed))
            }
        }
    }
}

pub(crate) struct ChunkfulRxStreamProducer {
    rx: tokio::sync::mpsc::Receiver<bytes::Bytes>,
}

impl<D> wasmtime::component::StreamProducer<D> for ChunkfulRxStreamProducer {
    type Item = u8;

    type Buffer = std::io::Cursor<bytes::Bytes>;

    fn poll_produce<'a>(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        store: wasmtime::StoreContextMut<'a, D>,
        mut destination: wasmtime::component::Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> std::task::Poll<anyhow::Result<wasmtime::component::StreamResult>> {
        use std::task::Poll;
        use wasmtime::component::StreamResult;

        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        let remaining = destination.remaining(store);
        if remaining.is_some_and(|r| r == 0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let recv = self.get_mut().rx.poll_recv(cx);
        match recv {
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(v)) => {
                let curses = std::io::Cursor::new(v);
                destination.set_buffer(curses);
                Poll::Ready(Ok(StreamResult::Completed))
            }
        }
    }
}


pub(crate) struct ChunkfulSyncRxStreamProducer {
    rx: std::sync::mpsc::Receiver<bytes::Bytes>,
}

impl<D> wasmtime::component::StreamProducer<D> for ChunkfulSyncRxStreamProducer {
    type Item = u8;

    type Buffer = std::io::Cursor<bytes::Bytes>;

    fn poll_produce<'a>(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        store: wasmtime::StoreContextMut<'a, D>,
        mut destination: wasmtime::component::Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> std::task::Poll<anyhow::Result<wasmtime::component::StreamResult>> {
        use std::task::Poll;
        use wasmtime::component::StreamResult;

        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }

        let remaining = destination.remaining(store);
        if remaining.is_some_and(|r| r == 0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let try_recv = self.get_mut().rx.try_recv();
        match try_recv {
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Poll::Ready(Ok(StreamResult::Dropped)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Poll::Pending,
            Ok(v) => {
                let curses = std::io::Cursor::new(v);
                destination.set_buffer(curses);
                Poll::Ready(Ok(StreamResult::Completed))
            }
        }
    }
}

pub(crate) struct RxFutureProducer<T> {
    rx: tokio::sync::oneshot::Receiver<T>,
}

impl<T: Send + Sync + 'static, D> wasmtime::component::FutureProducer<D> for RxFutureProducer<T> {
    type Item = T;

    fn poll_produce(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        _store: wasmtime::StoreContextMut<D>,
        _finish: bool,
    ) -> std::task::Poll<anyhow::Result<Option<Self::Item>>> {
        use std::task::Poll;
        use std::future::Future;

        let pinned_rx = std::pin::Pin::new(&mut self.get_mut().rx);
        match pinned_rx.poll(cx) {
            Poll::Ready(Err(e)) => Poll::Ready(Err(anyhow::anyhow!("{e:#}"))),
            Poll::Ready(Ok(cols)) => Poll::Ready(Ok(Some(cols))),
            Poll::Pending => Poll::Pending,
        }
    }
}
