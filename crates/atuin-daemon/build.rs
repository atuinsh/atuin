use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use protox::prost::Message;

fn main() -> std::io::Result<()> {
    let git = |args: &[&str]| {
        let output = Command::new("git").args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };

    // `grpc::versioning::BUILD_STR`, as semver build metadata so it still parses as a version.
    let version = env!("CARGO_PKG_VERSION");
    let build = git(&["rev-parse", "HEAD"])
        .map_or_else(|| version.to_owned(), |sha| format!("{version}+{sha}"));
    println!("cargo::rustc-env=ATUIN_BUILD_SHA={build}");

    println!("cargo::rerun-if-changed=proto");
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    for name in
        ["HEAD", "logs/HEAD", "packed-refs", "reftable"].into_iter().chain(branch.as_deref())
    {
        if let Some(path) =
            git(&["rev-parse", "--git-path", name]).filter(|p| Path::new(p).exists())
        {
            println!("cargo::rerun-if-changed={path}");
        }
    }

    let proto_paths = [
        "proto/common.proto",
        "proto/history.proto",
        "proto/search.proto",
        "proto/ai/agent.proto",
        "proto/ai/session.proto",
        "proto/userio.proto",
    ];
    let proto_include_dirs = ["proto"];

    let file_descriptors = protox::compile(proto_paths, proto_include_dirs).unwrap();

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
