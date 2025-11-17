pub mod client;
mod host;
mod types;

use client::ClientFactory;
use spin_factor_otel::OtelFactorState;
use spin_factor_outbound_networking::{
    config::allowed_hosts::OutboundAllowedHosts, OutboundNetworkingFactor,
};
use spin_factors::{
    anyhow, ConfigureAppContext, Factor, PrepareContext, RuntimeFactors,
    SelfInstanceBuilder,
};
use std::sync::Arc;

pub struct OutboundPgFactor<CF = crate::client::PooledTokioClientFactory> {
    _phantom: std::marker::PhantomData<CF>,
}

impl<CF: ClientFactory> Factor for OutboundPgFactor<CF> {
    type RuntimeConfig = ();
    type AppState = Arc<CF>;
    type InstanceBuilder = InstanceState<CF>;

    fn init(&mut self, ctx: &mut impl spin_factors::InitContext<Self>) -> anyhow::Result<()> {
        ctx.link_bindings(spin_world::v1::postgres::add_to_linker::<_, PgFactorData<CF>>)?;
        ctx.link_bindings(spin_world::v2::postgres::add_to_linker::<_, PgFactorData<CF>>)?;
        ctx.link_bindings(
            spin_world::spin::postgres3_0_0::postgres::add_to_linker::<_, PgFactorData<CF>>,
        )?;
        ctx.link_bindings(
            spin_world::spin::postgres4_1_0::postgres::add_to_linker::<_, PgFactorData<CF>>,
        )?;
        Ok(())
    }

    fn configure_app<T: RuntimeFactors>(
        &self,
        _ctx: ConfigureAppContext<T, Self>,
    ) -> anyhow::Result<Self::AppState> {
        Ok(Arc::new(CF::default()))
    }

    fn prepare<T: RuntimeFactors>(
        &self,
        mut ctx: PrepareContext<T, Self>,
    ) -> anyhow::Result<Self::InstanceBuilder> {
        let allowed_hosts = ctx
            .instance_builder::<OutboundNetworkingFactor>()?
            .allowed_hosts();
        let otel = OtelFactorState::from_prepare_context(&mut ctx)?;

        Ok(InstanceState {
            allowed_hosts,
            client_factory: ctx.app_state().clone(),
            connections: Default::default(),
            otel,
        })
    }
}

impl<C> Default for OutboundPgFactor<C> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<C> OutboundPgFactor<C> {
    pub fn new() -> Self {
        Self::default()
    }
}

pub struct InstanceState<CF: ClientFactory> {
    allowed_hosts: OutboundAllowedHosts,
    client_factory: Arc<CF>,
    connections: spin_resource_table::Table<CF::Client>,
    otel: OtelFactorState,
}

impl<CF: ClientFactory> SelfInstanceBuilder for InstanceState<CF> {}

// impl<CF: ClientFactory> spin_core::wasmtime::component::HasData for InstanceState<CF> {
//     type Data<'a> = <FactorData<OutboundPgFactor> as spin_core::wasmtime::component::HasData>::Data<'a>;
// }

pub struct PgFactorData<CF: ClientFactory>(OutboundPgFactor<CF>);

impl<CF: ClientFactory> spin_core::wasmtime::component::HasData for PgFactorData<CF> {
    type Data<'a> = &'a mut InstanceState<CF>;
}

impl<CF: ClientFactory> spin_core::wasmtime::component::HasData for InstanceState<CF> {
    type Data<'a> = &'a mut InstanceState<CF>;
}

use spin_core::wasmtime;
use spin_world::spin::postgres4_1_0::postgres::{self as v4};

impl<CF: ClientFactory> spin_world::spin::postgres4_1_0::postgres::HostConnectionWithStore for PgFactorData<CF> {
    async fn open_async<T>(accessor: &wasmtime::component::Accessor<T,Self>, address:wasmtime::component::__internal::String,) -> Result<wasmtime::component::Resource<v4::Connection>, v4::Error> {

        // This skips address permission checks because POC/gaining understanding

        let cf = accessor.with(|mut access| {
            let host = access.get(); 
            host.client_factory.clone()
        });
        let client = cf
            .get_client(&address)
            .await
            .map_err(|e| v4::Error::ConnectionFailed(format!("{e:?}")))?;
        let rsrc = accessor.with(|mut access| {
            let host = access.get();
            host.connections
                .push(client)
                .map_err(|_| v4::Error::ConnectionFailed("too many connections".into()))
                .map(wasmtime::component::Resource::new_own)
        });
        rsrc
    }
    
    async fn query_async<T>(
        accessor: &wasmtime::component::Accessor<T, Self>,
        self_: wasmtime::component::Resource<v4::Connection>,
        statement: String,
        params: Vec<v4::ParameterValue>
    ) -> Result<(
        wasmtime::component::FutureReader<Vec<v4::Column>>,
        wasmtime::component::StreamReader<v4::Row>
    ), v4::Error> {
        use wasmtime::AsContextMut;
        use client::Client;

        let client = accessor.with(|mut access| {
            let host = access.get(); 
            let cli = host.connections.get(self_.rep()).unwrap();
            cli.clone()
        });

        let (col_rx, mut results_rx) = client.query_async(statement, params).await?;

        let (tx, rx) = tokio::sync::mpsc::channel::<v4::Row>(100);
        let row_producer = RowProducer { rx };
        let col_producer = ColumnProducer { rx: col_rx };

        let (fr, sr) = accessor.with(|mut access| {
            let fr = wasmtime::component::FutureReader::new(access.instance(), access.as_context_mut(), col_producer);
            let sr = wasmtime::component::StreamReader::new(access.instance(), access.as_context_mut(), row_producer);
            (fr, sr)
        });

        tokio::task::spawn(async move {
            use futures::StreamExt;
            loop {
                // Yes there is probably a "connect stream to sink" thingy, don't @ me
                let Some(row) = results_rx.next().await else {
                    break;
                };
                tx.send(row).await.unwrap();
            }
        });

        Ok((fr, sr))
    }
}

struct RowProducer {
    rx: tokio::sync::mpsc::Receiver<v4::Row>,
}

impl<D> wasmtime::component::StreamProducer<D> for RowProducer {
    type Item = v4::Row;

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

struct ColumnProducer {
    rx: tokio::sync::oneshot::Receiver<Vec<v4::Column>>,
}

impl<D> wasmtime::component::FutureProducer<D> for ColumnProducer {
    type Item = Vec<v4::Column>;

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
