// The build script of `atuin-daemon` includes this file. The desktop capture helper takes its
// build string from the daemon's `BUILD_STR`, and the daemon stops a helper whose build string
// differs from its own.

/// Sets `ATUIN_BUILD_SHA` to the crate's version, the commit, and the features that add services,
/// and reruns the build script when the commit changes.
///
/// The commit and the features are semver build metadata, so the build string still parses as a
/// version. A build with desktop capture serves the `DesktopCapture` service, which a build of the
/// same commit without the feature lacks. The feature is in the build string so that a client
/// replaces a daemon that lacks the client's services.
fn emit_build_sha() {
    use std::fmt::Write as _;

    let git = |args: &[&str]| {
        let output = std::process::Command::new("git").args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };

    let sha = git(&["rev-parse", "HEAD"]);
    #[expect(
        clippy::disallowed_methods,
        reason = "build scripts can't use atuin_common, and Cargo sets the feature variables"
    )]
    let desktop_capture = std::env::var_os("CARGO_FEATURE_DESKTOP_CAPTURE").is_some();
    let metadata = sha.iter().map(String::as_str).chain(desktop_capture.then_some("desktop-capture"));

    let mut build = env!("CARGO_PKG_VERSION").to_owned();
    for (i, identifier) in metadata.enumerate() {
        let separator = if i == 0 { '+' } else { '.' };
        let _ = write!(build, "{separator}{identifier}");
    }
    println!("cargo::rustc-env=ATUIN_BUILD_SHA={build}");

    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    for name in
        ["HEAD", "logs/HEAD", "packed-refs", "reftable"].into_iter().chain(branch.as_deref())
    {
        if let Some(path) =
            git(&["rev-parse", "--git-path", name]).filter(|p| std::path::Path::new(p).exists())
        {
            println!("cargo::rerun-if-changed={path}");
        }
    }
}
