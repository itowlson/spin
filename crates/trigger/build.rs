fn main() -> std::io::Result<()> {
    tonic_build::configure()
        .out_dir("src/external")
        .compile(&["src/external/spinext.proto"], &["src/external"])?;
    Ok(())
}
