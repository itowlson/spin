use anyhow::Context;
use spin_core::wasmtime::Engine;
use super::error::convert_error;

/// Create a Wasmtime Engine with async support. We need a separate Engine
/// from the one Spin hosts app components in because, well, I don't know
/// why but a cabi_realloc error says we do.
pub fn create_host_engine() -> anyhow::Result<Engine> {
    let mut config = spin_core::wasmtime::Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    Engine::new(&config).map_err(convert_error).context("failed to create host component engine")
}
