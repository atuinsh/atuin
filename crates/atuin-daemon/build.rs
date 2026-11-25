use std::path::PathBuf;
use std::{env, fs};

use protox::prost::Message;

include!("build_sha.rs");

fn main() -> std::io::Result<()> {
    // The build string of `grpc::versioning::BUILD_STR`.
    emit_build_sha();
    println!("cargo::rerun-if-changed=proto");

    let mut proto_paths = vec![
        "proto/common.proto",
        "proto/history.proto",
        "proto/search.proto",
        "proto/userio.proto",
    ];
    if cfg!(feature = "desktop-capture") {
        proto_paths.push("proto/desktop_capture.proto");
    }
    if cfg!(feature = "ai") {
        proto_paths.extend(["proto/ai/agent.proto", "proto/ai/session.proto"]);
    }
    let proto_include_dirs = ["proto"];

    let file_descriptors = protox::compile(&proto_paths, proto_include_dirs).unwrap();

    #[expect(
        clippy::disallowed_methods,
        reason = "build scripts can't use atuin_common, and Cargo sets OUT_DIR for them"
    )]
    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR not set");
    let file_descriptor_path = PathBuf::from(out_dir).join("file_descriptor_set.bin");
    fs::write(&file_descriptor_path, file_descriptors.encode_to_vec()).unwrap();

    tonic_prost_build::configure()
        .build_server(true)
        .file_descriptor_set_path(&file_descriptor_path)
        .extern_path(".common.PyStyleIdxRange", "::atuin_common::range::PyStyleIdxRange")
        .extern_path(
            ".common.HighlightedText",
            "::atuin_common::string::highlighted::HighlightedTextProto",
        )
        .extern_path(".ai.agent.HarnessKind", "::atuin_client::ai_session::HarnessKind")
        .extern_path(".ai.agent.Tokens", "::atuin_common::harnesstools::session::Usage")
        .skip_protoc_run()
        .compile_protos(&proto_paths, &proto_include_dirs)
}
