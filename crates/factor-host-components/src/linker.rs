use std::{collections::HashMap};

use spin_core::InstancePre;
use spin_core::wasmtime::component::LinkerInstance;
use spin_factors::{
    InitContext,
    anyhow,
};

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
        let func_name_1 = func_name.clone();
        let func_name_2 = func_name.clone();
        if *is_async {
            link_concurrent_func::<T>(&mut linker_instance, inst_pre, func_name_1, func_name_2, hc2.clone(), interface2.clone());
        } else {
            link_func::<T>(&mut linker_instance, inst_pre, func_name_1, func_name_2, hc2.clone(), interface2.clone());
        }
    }
}

fn link_func<T: InitContext<HostComponentsFactor>>(linker_instance: &mut LinkerInstance<'_, T::StoreData>, inst_pre: InstancePre<T::StoreData>, func_name_1: String, func_name_2: String, hc3: LoadedHostComponent, interface3: ExportedInterface) {
    linker_instance.func_new_async(&func_name_1, move |mut store_ctx, _f, params, results| {
        let inst_pre2 = inst_pre.clone();
        let hc4 = hc3.clone();
        let interface4 = interface3.clone();
        let func_name_3 = func_name_2.clone();
        let fut = async move {
            let inst_state = T::get_data(store_ctx.data_mut());
            if let Some(func) = inst_state.get_handler(&interface4.name, &func_name_3) {
                func.call_async(&mut store_ctx, params, results).await.unwrap();
                return Ok(());
            }

            let hc_instance = inst_pre2.instantiate_async(&mut store_ctx).await.unwrap();
            let export_indices = snort_export_indices(&hc4, &hc_instance, &mut store_ctx).unwrap();

            let (_ei, fmap) = export_indices.get(&interface4.name).expect("itf name->fmap lookup failed");
            let fi = fmap.get(&func_name_3).expect("fun name->index lookup failed");
            let func = hc_instance.get_func(&mut store_ctx, fi).expect("fun index->THE THINGY lookup failed");
            let inst_state = T::get_data(store_ctx.data_mut());  // Have to get again because we can't hold the borrow across other things that want to borrow store_ctx
            inst_state.set_handler(&interface4.name, &func_name_3, func.clone());

            func.call_async(&mut store_ctx, params, results).await.unwrap();
            Ok(())
        };
        Box::new(fut)
    }).unwrap();
}

fn link_concurrent_func<T: InitContext<HostComponentsFactor>>(linker_instance: &mut LinkerInstance<'_, T::StoreData>, inst_pre: InstancePre<T::StoreData>, func_name_1: String, func_name_2: String, hc3: LoadedHostComponent, interface3: ExportedInterface) {
    linker_instance.func_new_concurrent(&func_name_1, move |accessor, _f, params, results| {
        let interface4 = interface3.clone();
        let hc4 = hc3.clone();
        let func = accessor.with(|mut access| {
            use spin_core::wasmtime::AsContextMut;

            let inst_state = T::get_data(access.data_mut());
            if let Some(func) = inst_state.get_handler(&interface4.name, &func_name_2) {
                return func;
            }

            let tokio_rt_cb = tokio::runtime::Handle::current();
            let hc_instance = tokio::task::block_in_place(|| tokio_rt_cb.block_on(inst_pre.instantiate_async(access.as_context_mut()))).unwrap();
            let export_indices = snort_export_indices(&hc4, &hc_instance, access.as_context_mut()).unwrap();
            let (_ei, fmap) = export_indices.get(&interface4.name).expect("itf name->fmap lookup failed");
            let fi = fmap.get(&func_name_2).expect("fun name->index lookup failed");
            let func = hc_instance.get_func(access.as_context_mut(), fi).expect("fun index->THE THINGY lookup failed");
            let inst_state = T::get_data(access.data_mut());
            inst_state.set_handler(&interface4.name, &func_name_2, func);
            func
        });
        let fut = async move {
            func.call_concurrent(accessor, params, results).await.unwrap();
            Ok(())
        };
        Box::pin(fut)
    }).unwrap();
}

fn snort_export_indices(loaded: &LoadedHostComponent, instance: &spin_core::Instance, mut store: impl spin_core::wasmtime::AsContextMut) -> anyhow::Result<HashMap<String, (spin_core::wasmtime::component::ComponentExportIndex, HashMap<String, spin_core::wasmtime::component::ComponentExportIndex>)>> {
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

    // for (itf, (_iindex, fmap)) in &export_indices {
    //     eprintln!("--- ITF {itf} ---");
    //     for (fname, fin) in fmap {
    //         eprintln!("* {fname} (idx={fin:?})");
    //     }
    // }

    Ok(export_indices)
}
