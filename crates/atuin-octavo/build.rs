use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=proto");
    println!("cargo::rerun-if-changed=migrations");

    let descriptors = protox::compile(["atuin/api/v1/hub_service.proto"], ["proto"])?;
    tonic_prost_build::configure().build_server(false).compile_fds(descriptors)?;

    Ok(())
}
