use std::{env, fs, path::PathBuf};

use protox::prost::Message;

fn main() -> std::io::Result<()> {
    let proto_paths = ["proto/history.proto"];
    let proto_include_dirs = ["proto"];

    let file_descriptors = protox::compile(proto_paths, proto_include_dirs).unwrap();

    let file_descriptor_path = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"))
        .join("file_descriptor_set.bin");
    fs::write(&file_descriptor_path, file_descriptors.encode_to_vec()).unwrap();

    tonic_prost_build::configure()
        .build_server(true)
        .file_descriptor_set_path(&file_descriptor_path)
        .skip_protoc_run()
        .compile_protos(&proto_paths, &proto_include_dirs)
}
