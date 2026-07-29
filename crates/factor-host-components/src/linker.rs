use std::{collections::HashMap};

use spin_core::Instance;
use spin_core::{InstancePre, wasmtime::StoreContextMut};
use spin_core::wasmtime::component::LinkerInstance;
use spin_factors::{
    InitContext,
    anyhow,
};
use spin_core::wasmtime::AsContextMut;

use crate::{HostComponentsFactor, SharedService, loader::{ExportedInterface, LoadedHostComponent}};

pub struct HostComponentInstancePre<SD: 'static> {
    pub instance_pre: spin_core::InstancePre<SD>,
}

pub fn link_interface<T: InitContext<HostComponentsFactor>>(ctx: &mut T, hc: &LoadedHostComponent, shared_instance_pre: &SharedService<T::StoreData>, interface: &ExportedInterface) {
    let hc2 = hc.clone();
    let interface2 = interface.clone();
    let inst_pre_ur = shared_instance_pre.instance_pre();
    let mut linker_instance = ctx.linker().instance(&interface.name).unwrap();
    for (func_name, is_async) in &interface.functions {
        let inst_pre = inst_pre_ur.clone();
        if *is_async {
            link_concurrent_func::<T>(&mut linker_instance, inst_pre, func_name, hc2.clone(), interface2.clone());
        } else {
            link_func::<T>(&mut linker_instance, inst_pre, func_name, hc2.clone(), interface2.clone());
        }
    }
}

fn link_func<T: InitContext<HostComponentsFactor>>(linker_instance: &mut LinkerInstance<'_, T::StoreData>, inst_pre: InstancePre<T::StoreData>, func_name: &str, hc3: LoadedHostComponent, interface3: ExportedInterface) {
    let func_name_2 = func_name.to_string();
    linker_instance.func_new_async(func_name, move |mut store_ctx, _f, params, results| {
        let inst_pre2 = inst_pre.clone();
        let hc4 = hc3.clone();
        let func_name_3 = func_name_2.clone();
        let interface4 = interface3.clone();
        let fut = async move {
            let func = match T::get_data(store_ctx.data_mut()).get_handler(&interface4.name, &func_name_3) {
                crate::ExistingFuncMapping::Func(func) => func,
                crate::ExistingFuncMapping::Instance(instance) => {
                    let (_, func) = get_func_from_instance::<T>(&mut store_ctx, instance, hc4, &interface4.name, &func_name_3);
                    T::get_data(store_ctx.data_mut()).set_handler(&interface4.name, &func_name_3, instance, func.clone());
                    func
                },
                crate::ExistingFuncMapping::None => {
                    let (instance, func) = get_func::<T>(&mut store_ctx, &inst_pre2, hc4, &interface4.name, &func_name_3).await;
                    T::get_data(store_ctx.data_mut()).set_handler(&interface4.name, &func_name_3, instance, func.clone());
                    func
                }
            };

            func.call_async(&mut store_ctx, params, results).await.unwrap();
            Ok(())
        };
        Box::new(fut)
    }).unwrap();
}

async fn get_func<T: InitContext<HostComponentsFactor>>(store_ctx: &mut StoreContextMut<'_, T::StoreData>, instance_pre: &InstancePre<T::StoreData>, host_component: LoadedHostComponent, interface_name: &str, func_name: &str) -> (spin_core::wasmtime::component::Instance, spin_core::wasmtime::component::Func) {
    let hc_instance = instance_pre.instantiate_async(&mut *store_ctx).await.unwrap();
    get_func_from_instance::<T>(store_ctx, hc_instance, host_component, interface_name, func_name)
}

fn get_func_from_instance<T: InitContext<HostComponentsFactor>>(store_ctx: &mut StoreContextMut<'_, T::StoreData>, instance: Instance, host_component: LoadedHostComponent, interface_name: &str, func_name: &str) -> (spin_core::wasmtime::component::Instance, spin_core::wasmtime::component::Func) {
    let export_indices = get_export_indices(&host_component, &instance, &mut *store_ctx).unwrap();

    let (_ei, fmap) = export_indices.get(interface_name).expect("itf name->fmap lookup failed");
    let fi = fmap.get(func_name).expect("fun name->index lookup failed");
    let func = instance.get_func(&mut *store_ctx, fi).expect("fun index->THE THINGY lookup failed");
    (instance, func)
}

fn link_concurrent_func<T: InitContext<HostComponentsFactor>>(linker_instance: &mut LinkerInstance<'_, T::StoreData>, instance_pre: InstancePre<T::StoreData>, func_name: &str, hc3: LoadedHostComponent, interface3: ExportedInterface) {
    let func_name_2 = func_name.to_string();
    linker_instance.func_new_concurrent(func_name, move |accessor, _f, params, results| {
        let interface4 = interface3.clone();
        let hc4 = hc3.clone();
        let func = accessor.with(|mut access| {
            match T::get_data(access.data_mut()).get_handler(&interface4.name, &func_name_2) {
                crate::ExistingFuncMapping::Func(func) => func,
                crate::ExistingFuncMapping::Instance(instance) => {
                    let (_, func) = get_func_from_instance::<T>(&mut access.as_context_mut(), instance, hc4, &interface4.name, &func_name_2);
                    T::get_data(access.data_mut()).set_handler(&interface4.name, &func_name_2, instance, func.clone());
                    func
                }
                crate::ExistingFuncMapping::None => {
                    let tokio_rt = tokio::runtime::Handle::current();
                    let (instance, func) = tokio::task::block_in_place(|| tokio_rt.block_on(get_func::<T>(&mut access.as_context_mut(), &instance_pre, hc4, &interface4.name, &func_name_2)));
                    T::get_data(access.data_mut()).set_handler(&interface4.name, &func_name_2, instance, func.clone());
                    func
                }
            }
        });
        let fut = async move {
            func.call_concurrent(accessor, params, results).await.unwrap();
            Ok(())
        };
        Box::pin(fut)
    }).unwrap();
}

fn get_export_indices(loaded: &LoadedHostComponent, instance: &spin_core::Instance, mut store: impl spin_core::wasmtime::AsContextMut) -> anyhow::Result<HashMap<String, (spin_core::wasmtime::component::ComponentExportIndex, HashMap<String, spin_core::wasmtime::component::ComponentExportIndex>)>> {
    use anyhow::anyhow;

    let mut export_indices = HashMap::new();

    for iface in &loaded.exported_interfaces {
        let iface_index = instance
            .get_export_index(&mut store, None, &iface.name)
            .ok_or_else(|| {
                anyhow!(
                    "host component '{}' missing expected export '{}'",
                    loaded.name,
                    iface.name
                )
            })?;

        let mut func_indices = HashMap::new();
        for (func_name, _) in &iface.functions {
            let func_index = instance
                .get_export_index(&mut store, Some(&iface_index), func_name)
                .ok_or_else(|| {
                    anyhow!(
                        "host component '{}' interface '{}' missing function '{}'",
                        loaded.name,
                        iface.name,
                        func_name
                    )
                })?;
            func_indices.insert(func_name.clone(), func_index);
        }

        export_indices.insert(iface.name.clone(), (iface_index, func_indices));
    }

    Ok(export_indices)
}
