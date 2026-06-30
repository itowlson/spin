pub fn convert_error(e: spin_core::wasmtime::Error) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}
