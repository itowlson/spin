use anyhow::Result;

use spin_app::DynamicHostComponent;
use spin_core::{Data, HostComponent, Linker};

use crate::{allowed_http_hosts::{parse_allowed_http_hosts, RuntimeHostAllowerFactory}, OutboundHttp};

pub struct OutboundHttpComponent {
    pub rhaf: Option<RuntimeHostAllowerFactory>,
}

pub const ALLOWED_HTTP_HOSTS_METADATA_KEY: &str = "allowed_http_hosts";

impl HostComponent for OutboundHttpComponent {
    type Data = OutboundHttp;

    fn add_to_linker<T: Send>(
        linker: &mut Linker<T>,
        get: impl Fn(&mut Data<T>) -> &mut Self::Data + Send + Sync + Copy + 'static,
    ) -> Result<()> {
        super::wasi_outbound_http::add_to_linker(linker, get)
    }

    fn build_data(&self) -> Self::Data {
        Default::default()
    }
}

impl DynamicHostComponent for OutboundHttpComponent {
    fn update_data(
        &self,
        data: &mut Self::Data,
        component: &spin_app::AppComponent,
    ) -> anyhow::Result<()> {
        // println!("sorting out allowed hosts stuff");
        let hosts = component.get_metadata(ALLOWED_HTTP_HOSTS_METADATA_KEY)?;
        let component_id = component.id();
        data.allowed_hosts = parse_allowed_http_hosts(&hosts, self.rhaf.as_ref().map(|r| (r.f)(component_id.to_owned())))?;
        Ok(())
    }
}
