//! gRPC layer which ensures that the client and the daemon are running the same version.
//!
//! This layer enables you to change the proto files freely, without worrying about `reserved`
//! fields.
//!
//! TL;DR:
//!
//!   - Change the protos freely, except for the `Status` and `Shutdown` RPCs, which a client uses
//!     to find and replace a daemon of another build.
//!   - There is no protocol version to bump. A client and a daemon talk only if their builds
//!     match.
//!
//! # Wire Compatibility
//!
//! Every client request carries the client's build in the [`BUILD_HEADER_KEY`] header and every
//! response carries the daemon's. The daemon refuses a request from another build before decoding
//! it (see [`StrictVersionService`]), so two builds never exchange a message they might disagree
//! on the shape of. Only `Status` and `Shutdown` pass between builds: they are how a client finds a
//! daemon of another build and replaces it.
//!
//! ## Different Package Managers
//!
//! There are many ways to install Atuin and consequently let's analyze which installation method
//! hits which code path.
//!
//! ### curl Install Script
//!
//! - If `daemon.autostart = true` (default when the daemon is enabled in the setup), the daemon
//!   will gracefully be restarted when the build changes. Wire incompatibility is a-ok in this
//!   case.
//! - If `daemon.autostart = false`, we will tell the user to restart Atuin.
//!
//! #### Homebrew
//!
//! <https://raw.githubusercontent.com/Homebrew/homebrew-core/master/Formula/a/atuin.rb> presents
//! the user with a homebrew service. This means the user can run `brew services start atuin` to run
//! a new atuin daemon.
//!
//! We will try to restart it via the `Shutdown` RPC. When the daemon attempts to game end, it will
//! terminate, and the client will immediately to try to run another daemon:
//!
//!   - Either `keep_alive` (on macOS launchd) wins, which means a new version of the daemon is
//!     spawned, and managed by launchd.
//!   - Or, more likely, the client-spawned daemon wins, which means a new version of the daemon is
//!     managed by the client.
//!
//! **In effect, if the user updates via brew, the next command we send to the daemon will hit that
//! build mismatch and the daemon will get restarted, one way or another.**
//!
//! The only exception to this rule is if they:
//!  - Ran `brew service start atuin`
//!  - And also said **no** to the daemon during setup.
//!
//! But in that case, we won't even try to talk to the daemon, so a mismatched version doesn't
//! matter.
//!
//! ### Debian
//!
//! <https://packages.debian.org/sid/amd64/atuin/filelist> is not managed by systemctl and is
//! therefore identical to the curl Install Script case above, governed entirely by
//! `enabled`/`autostart`.
//!
//! ### Ubuntu
//!
//! <https://packages.ubuntu.com/questing/amd64/atuin/filelist> is the same as the curl Install
//! Script case above.
//!
//! ### Fedora
//!
//! <https://packages.fedoraproject.org/pkgs/atuin/atuin/> seems identical to the curl Install
//! Script case.
//!
//! ### Arch
//!
//! <https://archlinux.org/packages/extra/x86_64/atuin/> is identical to the curl Install Script
//! case -- we manage the daemon.
//!
//! ### Alpine
//!
//! <https://pkgs.alpinelinux.org/package/edge/community/x86_64/atuin> -- the main package install
//! matches the curl Install Script case.
//!
//! ### Void
//!
//! <https://raw.githubusercontent.com/void-linux/void-packages/master/srcpkgs/atuin/template> --
//! binary-only: the template installs the binary, license, and completions, and the `srcpkgs/atuin`
//! dir contains only `template` (no `patches/`, no `files/`). Void is runit-based with no daemon
//! service, so `systemd_socket` is irrelevant and behavior matches the curl Install Script case.
//!
//! ### home-manager (Nix)
//!
//! <https://github.com/nix-community/home-manager/blob/master/modules/programs/atuin.nix> exposes
//! `programs.atuin.daemon.enable`. When enabled it generates a `systemd.user.services.atuin-daemon` +
//! `systemd.user.sockets.atuin-daemon` pair on Linux, or a `launchd.agents.atuin-daemon` on
//! macOS. Crucially, it also WRITES our config into `config.toml`:
//!
//!   - `daemon.enabled = true` (always),
//!   - `daemon.systemd_socket = true` on systemd, `false` on launchd,
//!   - `daemon.socket_path = $XDG_DATA_HOME/atuin/daemon.sock` on launchd only.
//!
//! It never sets `daemon.autostart`, so it stays `false`. That means the client ALWAYS takes the
//! bail-on-mismatch path -- it never sends `Shutdown` and never respawns the daemon itself.
//! Replacing the daemon is left to the service manager plus `home-manager switch`.
//!
//!   - On Linux (`systemd_socket = true`, `autostart = false`), the `.socket` listens on
//!     `%t/atuin.sock` (`$XDG_RUNTIME_DIR/atuin.sock`), exactly our socket-activation path. On a
//!     mismatch the client bails. A rebuild changes the unit's `ExecStart` store path, so
//!     activation (the default `sd-switch` backend) restarts the unit; the next connection then
//!     socket-activates the new binary.
//!   - On macOS (`systemd_socket = false`, `autostart = false`), the launchd agent uses a
//!     conditional `KeepAlive` (`Crashed = true; SuccessfulExit = false`), so launchd will NOT
//!     respawn a cleanly-exited daemon -- a stale-version daemon persists until `home-manager
//!     switch` reloads the agent (its `ProgramArguments` store path changes).
//!
//! Caveat: these settings only land if home-manager can write `config.toml` -- it is generated with
//! `force = forceOverwriteSettings` (default `false`), and since atuin rewrites its own config, a
//! pre-existing real `config.toml` blocks them unless `forceOverwriteSettings = true`.
//!
//! ### Gentoo
//!
//! The `app-shells/atuin` ebuild, behind the default-on `daemon` USE flag, installs two systemd
//! USER units via `systemd_douserunit` (it patches nothing and writes no config):
//! <https://github.com/gentoo/gentoo/blob/master/app-shells/atuin/files/atuin-daemon.socket> and
//! <https://github.com/gentoo/gentoo/blob/master/app-shells/atuin/files/atuin-daemon.service>.
//!
//! The socket listens on `%t/atuin.sock` (`$XDG_RUNTIME_DIR/atuin.sock`, our socket-activation
//! path) with `RemoveOnStop=true`. The service is pure socket-activation: `Requires=` the socket,
//! `ExecStart=atuin daemon` (the deprecated bare form; we warn in favor of `atuin daemon start`),
//! and it sets no `Restart=`, so systemd defaults to `Restart=no` and never supervise-restarts it.
//!
//! Because the ebuild sets no config, the user must set `daemon.systemd_socket = true` (and
//! `enabled = true`) themselves; the intended pairing is `systemd_socket = true` + `autostart =
//! false`. On a mismatch the client therefore bails and defers to systemd (autostart is
//! incompatible with `systemd_socket`).
//!
//! **In effect an `emerge` upgrade leaves the stale daemon running until it is stopped; the next
//! client connection then socket-activates a fresh daemon from the new binary.**
//!
//! ## Daemons From Before Builds Were Exchanged
//!
//! Clients are short-lived, so only a daemon outlives an upgrade. One from before builds were
//! exchanged sends no build header and acts on any request it can decode, so a client replaces it
//! before sending anything but `Status` and `Shutdown`.

use std::sync::LazyLock;
use std::task::{Context, Poll};

use futures::TryFutureExt;
use futures::future::{Either, MapOk, Ready, ready};
use http::HeaderValue;
use thiserror::Error;
use tonic::body::Body;
use tonic::metadata::{Ascii, MetadataMap, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Response, Status};
use tower::{Layer, Service};

/// The header carrying the sender's [`BUILD_STR`] on every request and response.
pub const BUILD_HEADER_KEY: &str = "x-atuin-build";
/// The build of Atuin a client or daemon runs: the Atuin version, plus `+` and the commit SHA when
/// built from a git checkout, e.g. `18.20.0+3f2a9c...`.
pub const BUILD_STR: &str = env!("ATUIN_BUILD_SHA");
const BUILD_HEADER_VALUE: HeaderValue = HeaderValue::from_static(BUILD_STR);
static BUILD_METADATA_VALUE: LazyLock<MetadataValue<Ascii>> =
    LazyLock::new(|| MetadataValue::from_static(BUILD_STR));

/// An error thrown when the version announced by the peer doesn't match our version.
#[derive(Debug, Clone, Error)]
pub enum VersionMatchError {
    #[error("the peer did not specify the x-atuin-build header")]
    Unannounced,
    #[error("the peer's version does not match ours. theirs={theirs}, ours={ours}")]
    Mismatched {
        theirs: String,
        ours: &'static str,
    },
    #[error("the peer's x-atuin-build header is not ASCII: {}", String::from_utf8_lossy(.theirs))]
    Malformed {
        theirs: Vec<u8>,
    },
}

impl VersionMatchError {
    /// Whether the peer runs a newer build than ours.
    #[must_use]
    pub fn peer_is_newer(&self) -> bool {
        match self {
            Self::Mismatched { theirs, ours } => matches!(
                (semver::Version::parse(theirs), semver::Version::parse(ours)),
                (Ok(peer), Ok(ours)) if peer.cmp_precedence(&ours).is_gt()
            ),
            Self::Unannounced | Self::Malformed { .. } => false,
        }
    }
}

/// Checks the build a peer announced on a reply or a refusal.
pub trait VersionedExt {
    /// Check whether the version of the peer matches the expected version.
    fn check_version(&self) -> Result<(), VersionMatchError>;
}

impl VersionedExt for MetadataMap {
    fn check_version(&self) -> Result<(), VersionMatchError> {
        let Some(build) = self.get(BUILD_HEADER_KEY) else {
            return Err(VersionMatchError::Unannounced);
        };
        if build == *BUILD_METADATA_VALUE {
            return Ok(());
        }
        let Ok(theirs) = build.to_str() else {
            return Err(VersionMatchError::Malformed {
                theirs: build.as_encoded_bytes().to_vec(),
            });
        };
        Err(VersionMatchError::Mismatched {
            theirs: theirs.to_owned(),
            ours: BUILD_STR,
        })
    }
}

impl VersionedExt for Status {
    fn check_version(&self) -> Result<(), VersionMatchError> {
        self.metadata().check_version()
    }
}

impl<T> VersionedExt for Response<T> {
    fn check_version(&self) -> Result<(), VersionMatchError> {
        self.metadata().check_version()
    }
}
/// Stamps a client's requests with its [`BUILD_STR`], which [`StrictVersionService`] checks.
#[derive(Clone, Copy, Debug, Default)]
pub struct StrictVersionInterceptor;

impl Interceptor for StrictVersionInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert(BUILD_HEADER_KEY, (*BUILD_METADATA_VALUE).clone());
        Ok(request)
    }
}

/// Wraps the daemon's services in a [`StrictVersionService`].
#[derive(Clone, Copy, Debug, Default)]
pub struct StrictVersionLayer {
    ungated: &'static [&'static str],
}

impl StrictVersionLayer {
    #[must_use]
    pub const fn new(ungated: &'static [&'static str]) -> Self {
        Self { ungated }
    }
}

impl<S> Layer<S> for StrictVersionLayer {
    type Service = StrictVersionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        StrictVersionService {
            inner,
            ungated: self.ungated,
        }
    }
}

/// A [`Service`] wrapper which refuses requests which do not match the build of the server, as well
/// as stamps responses with the build SHA of the daemon.
///
/// Refused responses are `FAILED_PRECONDITION`. Clients **must** compare their own header value
/// after the response, as implementations are allowed to emit their own `FAILED_PRECONDITION`s.
#[derive(Clone, Debug)]
pub struct StrictVersionService<S> {
    inner: S,
    ungated: &'static [&'static str],
}

impl<S> StrictVersionService<S> {
    #[must_use]
    pub const fn new(inner: S, ungated: &'static [&'static str]) -> Self {
        Self { inner, ungated }
    }

    fn stamp(mut response: http::Response<Body>) -> http::Response<Body> {
        response.headers_mut().insert(BUILD_HEADER_KEY, BUILD_HEADER_VALUE);
        response
    }
}

type Stamped<F> = MapOk<F, fn(http::Response<Body>) -> http::Response<Body>>;

impl<S, B> Service<http::Request<B>> for StrictVersionService<S>
where
    S: Service<http::Request<B>, Response = http::Response<Body>>,
{
    type Response = http::Response<Body>;
    type Error = S::Error;
    type Future = Either<Ready<Result<Self::Response, S::Error>>, Stamped<S::Future>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let theirs = request.headers().get(BUILD_HEADER_KEY);

        if self.ungated.contains(&request.uri().path())
            || theirs.is_some_and(|build| build == BUILD_STR)
        {
            return Either::Right(self.inner.call(request).map_ok(Self::stamp as fn(_) -> _));
        }

        let refusal = match theirs {
            Some(theirs) => format!(
                "daemon is build {BUILD_STR}, client is build {}",
                String::from_utf8_lossy(theirs.as_bytes())
            ),
            None => format!("daemon is build {BUILD_STR}, client sent no build"),
        };
        Either::Left(ready(Ok(Self::stamp(Status::failed_precondition(refusal).into_http()))))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The check's report of a daemon of build `theirs`.
    fn mismatched(theirs: &str) -> VersionMatchError {
        VersionMatchError::Mismatched {
            theirs: theirs.to_owned(),
            ours: BUILD_STR,
        }
    }

    #[rstest]
    #[case::older(mismatched("0.0.0"), false)]
    #[case::newer(mismatched("999.0.0"), true)]
    // `zzz` sorts after any SHA, so this daemon would look newer if commits were ordered.
    #[case::other_commit_of_ours(mismatched(&format!("{}+zzz", crate::VERSION)), false)]
    #[case::unparsable(mismatched("dev"), false)]
    #[case::malformed(VersionMatchError::Malformed { theirs: vec![0xff] }, false)]
    #[case::predates_builds(VersionMatchError::Unannounced, false)]
    fn only_a_newer_build_is_left_running(
        #[case] mismatch: VersionMatchError,
        #[case] newer: bool,
    ) {
        assert_eq!(mismatch.peer_is_newer(), newer);
    }
}
