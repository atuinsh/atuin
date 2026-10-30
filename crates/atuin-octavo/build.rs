use std::error::Error;
use std::path::PathBuf;
use std::{env, fs};

use protox::prost::Message as _;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=proto");
    println!("cargo::rerun-if-changed=migrations");

    let descriptors = protox::compile(["atuin/api/v1/hub_service.proto"], ["proto"])?;
    #[expect(
        clippy::disallowed_methods,
        reason = "build scripts can't use atuin_common, and Cargo sets OUT_DIR for them"
    )]
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?);
    // Read by the tests that hold atuin's `HarnessKind` and `Usage` to the proto they stand in for.
    fs::write(out_dir.join("hub_service.bin"), descriptors.encode_to_vec())?;
    tonic_prost_build::configure()
        .build_server(false)
        .extern_path(".atuin.api.v1.HarnessKind", "::atuin_client::ai_session::HarnessKind")
        .extern_path(".atuin.api.v1.TokenUsage", "::atuin_common::harnesstools::session::Usage")
        // The heartbeat beside a whole History is empty, so the event is always lopsided, and
        // boxing the entry would only add an allocation to each one streamed.
        .type_attribute(
            ".atuin.api.v1.WatchHistoryResponse.event",
            "#[allow(clippy::large_enum_variant)]",
        )
        .compile_fds(descriptors)?;

    Ok(())
}
