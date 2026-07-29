use std::{collections::HashMap};

use spin_core::Instance;
use spin_core::{InstancePre, wasmtime::StoreContextMut};
use spin_core::wasmtime::component::{ComponentExportIndex, LinkerInstance};
use spin_factors::{
    InitContext,
};
use spin_core::wasmtime::AsContextMut;

use crate::{HostComponentsFactor, SharedInstancePre, loader::{ExportedInterface}};

pub struct HostComponentInstancePre<SD: 'static> {
    pub instance_pre: spin_core::InstancePre<SD>,
    pub exports: HashMap<String, (ComponentExportIndex, HashMap<String, ComponentExportIndex>)>,
}

pub fn link_interface<T: InitContext<HostComponentsFactor>>(ctx: &mut T, shared_instance_pre: &SharedInstancePre<T::StoreData>, interface: &ExportedInterface) {
    let inst_pre = shared_instance_pre.instance_pre();
    let mut linker_instance = ctx.linker().instance(&interface.name).unwrap();
    for (func_name, is_async) in &interface.functions {
        let inst_pre = inst_pre.clone();
        let export_index = shared_instance_pre.export(&interface.name, func_name).expect("no func");
        if *is_async {
            link_concurrent_func::<T>(&mut linker_instance, inst_pre, &interface.name, func_name, export_index);
        } else {
            link_func::<T>(&mut linker_instance, inst_pre, &interface.name, func_name, export_index);
        }
    }
}

fn link_func<T: InitContext<HostComponentsFactor>>(linker_instance: &mut LinkerInstance<'_, T::StoreData>, inst_pre: InstancePre<T::StoreData>, interface_name: &str, func_name: &str, func_export_index: ComponentExportIndex) {
    let func_name_copy = func_name.to_string(); // We need one to borrow and one to move
    let interface_name = interface_name.to_string();
    linker_instance.func_new_async(func_name, move |mut store_ctx, _f, params, results| {
        let inst_pre = inst_pre.clone();
        let func_name = func_name_copy.clone();
        let interface_name = interface_name.clone();
        let func_export_index = func_export_index.clone();
        let fut = async move {
            let func = match T::get_data(store_ctx.data_mut()).get_handler(&interface_name, &func_name) {
                crate::ExistingFuncMapping::Func(func) => func,
                crate::ExistingFuncMapping::Instance(instance) => {
                    let (_, func) = get_func_from_instance(&mut store_ctx, instance, func_export_index);
                    T::get_data(store_ctx.data_mut()).set_handler(&interface_name, &func_name, instance, func.clone());
                    func
                },
                crate::ExistingFuncMapping::None => {
                    let (instance, func) = get_func(&mut store_ctx, &inst_pre, func_export_index).await;
                    T::get_data(store_ctx.data_mut()).set_handler(&interface_name, &func_name, instance, func.clone());
                    func
                }
            };

            func.call_async(&mut store_ctx, params, results).await.unwrap();
            Ok(())
        };
        Box::new(fut)
    }).unwrap();
}

fn link_concurrent_func<T: InitContext<HostComponentsFactor>>(linker_instance: &mut LinkerInstance<'_, T::StoreData>, instance_pre: InstancePre<T::StoreData>, interface_name: &str, func_name: &str, func_export_index: ComponentExportIndex) {
    let func_name_copy = func_name.to_string(); // We need one to borrow and one to move
    let interface_name = interface_name.to_string();
    linker_instance.func_new_concurrent(func_name, move |accessor, _f, params, results| {
        let func_name = func_name_copy.clone();
        let func = accessor.with(|mut access| {
            match T::get_data(access.data_mut()).get_handler(&interface_name, &func_name) {
                crate::ExistingFuncMapping::Func(func) => func,
                crate::ExistingFuncMapping::Instance(instance) => {
                    let (_, func) = get_func_from_instance(&mut access.as_context_mut(), instance, func_export_index);
                    T::get_data(access.data_mut()).set_handler(&interface_name, &func_name, instance, func.clone());
                    func
                }
                crate::ExistingFuncMapping::None => {
                    let tokio_rt = tokio::runtime::Handle::current();
                    let (instance, func) = tokio::task::block_in_place(|| tokio_rt.block_on(get_func(&mut access.as_context_mut(), &instance_pre, func_export_index)));
                    T::get_data(access.data_mut()).set_handler(&interface_name, &func_name, instance, func.clone());
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

async fn get_func<SD>(store_ctx: &mut StoreContextMut<'_, SD>, instance_pre: &InstancePre<SD>, func_export_index: ComponentExportIndex) -> (spin_core::wasmtime::component::Instance, spin_core::wasmtime::component::Func) {
    let instance = instance_pre.instantiate_async(&mut *store_ctx).await.unwrap();
    get_func_from_instance(store_ctx, instance, func_export_index)
}

fn get_func_from_instance<SD>(store_ctx: &mut StoreContextMut<'_, SD>, instance: Instance, func_export_index: ComponentExportIndex) -> (spin_core::wasmtime::component::Instance, spin_core::wasmtime::component::Func) {
    let func = instance.get_func(&mut *store_ctx, func_export_index).expect("func index->func lookup failed");
    (instance, func)
}
