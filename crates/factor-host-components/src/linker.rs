// use std::collections::HashMap;

// use spin_core::wasmtime::component::ComponentExportIndex;

pub struct HostComponentInstance<SD: 'static> {
    pub instance_pre: spin_core::InstancePre<SD>,
    // /// Cached export indices: interface_name -> (interface_index, {func_name -> func_index})
    // pub export_indices: HashMap<String, (ComponentExportIndex, HashMap<String, ComponentExportIndex>)>,
}
