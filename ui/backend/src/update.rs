// While technically using a "self update" crate, we can actually use the same method
// for managing a CLI install. Neat!
// This should still be locked to the same version as the UI. Drift there could lead to issues.
// In the future we can follow semver and allow for minor version drift.

// If you'd like to follow the conventions of your OS, distro, etc, then I would suggest
// following the CLI install instructions. This is intended to streamline install UX
use eyre::{eyre, Result};
use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

pub fn install(version: &str, path: &str) -> Result<()> {
    let dir = std::path::PathBuf::from(path);
    std::fs::create_dir_all(path)?;
    let bin = dir.join("atuin");

    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner("atuinsh")
        .repo_name("atuin")
        .build()?
        .fetch()?;

    let release = releases
        .iter()
        .find(|r| r.version == version)
        .ok_or_else(|| eyre!("No release found for version: {}", version))?;

    let asset = release
        .asset_for(&self_update::get_target(), None)
        .ok_or_else(|| eyre!("No asset found for target"))?;

    let tmp_dir = tempfile::Builder::new().prefix("atuin").tempdir()?;
    let tmp_tarball_path = tmp_dir.path().join(&asset.name);
    let tmp_tarball = std::fs::File::create(&tmp_tarball_path)?;
    println!("{:?}", tmp_tarball_path);

    self_update::Download::from_url(&asset.download_url).download_to(&tmp_tarball)?;

    let root = asset.name.replace(".tar.gz", "");
    let bin_name = std::path::PathBuf::from(format!("{}/atuin", root,));

    self_update::Extract::from_source(&tmp_tarball_path)
        .archive(self_update::ArchiveKind::Tar(Some(
            self_update::Compression::Gz,
        )))
        .extract_file(&bin, &bin_name)?;

    Ok(())
}
