use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=proto");
    println!("cargo::rerun-if-changed=migrations");

    let descriptors = protox::compile(["atuin/api/v1/hub_service.proto"], ["proto"])?;
    tonic_prost_build::configure()
        .build_server(false)
        // The heartbeat beside a whole History is empty, so the event is always lopsided, and
        // boxing the entry would only add an allocation to each one streamed.
        .type_attribute(
            ".atuin.api.v1.WatchHistoryResponse.event",
            "#[allow(clippy::large_enum_variant)]",
        )
        .compile_fds(descriptors)?;

    Ok(())
}
