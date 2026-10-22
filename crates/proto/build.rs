fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts run single-threaded for this purpose.
    unsafe { std::env::set_var("PROTOC", protoc) };

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(out_dir.join("payment_gateway_v1_descriptor.bin"))
        .compile_protos(
            &["../../proto/payment_gateway/v1/gateway.proto"],
            &["../../proto"],
        )?;
    Ok(())
}
