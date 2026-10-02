use std::error::Error;
use std::path::PathBuf;
use std::{env, fs};

use protox::prost::Message;

fn main() -> Result<(), Box<dyn Error>> {
    let proto_paths = [
        "proto/atuin/api/v1/common.proto",
        "proto/atuin/api/v1/history.proto",
        "proto/atuin/api/v1/hub_service.proto",
    ];
    let proto_include_dirs = ["proto"];

    let file_descriptors = protox::compile(proto_paths, proto_include_dirs)?;

    let file_descriptor_path = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR not set")?)
        .join("file_descriptor_set.bin");
    fs::write(&file_descriptor_path, file_descriptors.encode_to_vec())?;

    tonic_prost_build::configure()
        .build_server(true)
        .server_mod_attribute("atuin.api.v1", "#[cfg(test)]")
        .file_descriptor_set_path(&file_descriptor_path)
        .skip_protoc_run()
        .compile_protos(&proto_paths, &proto_include_dirs)?;

    Ok(())
}
