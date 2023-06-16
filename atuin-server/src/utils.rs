use eyre::Result;
use semver::{Version, VersionReq};

pub fn client_version_min(user_agent: &str, req: &str) -> Result<bool> {
    if user_agent.is_empty() {
        return Ok(false);
    }

    let version = user_agent.replace("atuin/", "");

    let req = VersionReq::parse(req)?;
    let version = Version::parse(version.as_str())?;

    Ok(req.matches(&version))
}
