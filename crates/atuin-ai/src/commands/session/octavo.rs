//! Thread capture and Octavo sharing status for `atuin threads status`.

use std::io::{self, Write};

use atuin_client::ai_session::Session;
use atuin_client::settings::Settings;
use atuin_daemon::grpc::octavo::pb;
use atuin_daemon::{AiClient, ThreadsClient};
use eyre::{Result, eyre};
use jiff::Timestamp;
use serde::Serialize;
use strum_macros::IntoStaticStr;

use super::Style;

/// Print capture and Octavo sharing status for the thread matching `selector`.
pub(super) async fn status(
    settings: &Settings,
    client: &mut AiClient,
    selector: &str,
    style: Style,
) -> Result<()> {
    let Some(session) = super::find(client, selector).await? else {
        return Status::uncaptured(settings).print(style);
    };
    let shares = ThreadsClient::from_settings(settings)
        .await?
        .shares(session.sharing().clone())
        .await
        .map_err(|status| eyre!("{}", status.message()))?;
    Status::of(settings, &session, shares)?.print(style)
}

fn write_status(out: &mut dyn Write, status: &Status) -> io::Result<()> {
    if matches!(status.capture, Capture::Off) {
        writeln!(
            out,
            "Thread capture is off. `atuin setup --only --threads --daemon` turns it on."
        )?;
    }
    let (Some(thread), Some(shares)) = (&status.thread, &status.octavo) else {
        return writeln!(out, "Not captured yet.");
    };
    let noun = if thread.messages == 1 {
        "message"
    } else {
        "messages"
    };
    let age = super::age(thread.updated);
    writeln!(out, "Captured: {} {noun}, the newest {age}.", thread.messages)?;
    write_plain(out, shares)
}

fn write_plain(out: &mut dyn Write, shares: &Shares) -> io::Result<()> {
    match shares.upload {
        Upload::Off => {
            return writeln!(
                out,
                "Octavo is off, so this machine shares no thread, and can't say who this one was \
                 shared with before. `atuin setup --only --hub` turns it on."
            );
        }
        Upload::LoggedOut => {
            return writeln!(
                out,
                "Logged out, so this machine shares no thread, and can't say who this one was \
                 shared with before. Run `atuin login`."
            );
        }
        Upload::On => {}
    }

    if shares.private {
        let closing: Vec<&str> = shares
            .orgs
            .iter()
            .filter(|org| matches!(org.state, ShareState::Closing))
            .map(|org| org.slug.as_str())
            .collect();
        match (closing.is_empty(), shares.revoking_links) {
            (true, false) => writeln!(
                out,
                "Private: closed to every organization this machine shared it with, and its links \
                 revoked."
            )?,
            (true, true) => writeln!(
                out,
                "Private once Atuin Hub revokes its links, which is still pending. Until then a \
                 link may still open it."
            )?,
            (false, false) => writeln!(
                out,
                "Private once Atuin Hub closes it to {}, which is still pending.",
                closing.join(", ")
            )?,
            (false, true) => writeln!(
                out,
                "Private once Atuin Hub closes it to {} and revokes its links, which is still \
                 pending. Until then a link may still open it.",
                closing.join(", ")
            )?,
        }
    }
    if shares.orgs.is_empty() {
        writeln!(out, "Not shared with any organization.")?;
    } else {
        writeln!(out, "{:<24} STATE", "ORGANIZATION")?;
        for org in &shares.orgs {
            writeln!(out, "{:<24} {}", org.slug, <&str>::from(org.state))?;
        }
    }

    if shares.dirs.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    writeln!(out, "Directories it ran in, and who each one's config shares with:")?;
    for dir in &shares.dirs {
        let orgs = if dir.orgs.is_empty() {
            "nobody".to_owned()
        } else {
            dir.orgs.join(", ")
        };
        writeln!(out, "  {}  {orgs}", super::sanitize(&dir.dir))?;
    }
    Ok(())
}

/// Capture and Octavo sharing status for a thread; serialized directly for `--style json`.
#[derive(Debug, Serialize)]
pub(in crate::commands) struct Status {
    capture: Capture,
    /// `None` if the thread has not been captured yet.
    thread: Option<Captured>,
    /// `None` if the thread has not been captured yet.
    octavo: Option<Shares>,
}

impl Status {
    fn uncaptured(settings: &Settings) -> Self {
        Self {
            capture: Capture::of(settings),
            thread: None,
            octavo: None,
        }
    }

    /// Build status for a captured `session`, treating `shares = None` as Octavo disabled.
    pub(in crate::commands) fn of(
        settings: &Settings,
        session: &Session,
        shares: Option<pb::ThreadShares>,
    ) -> Result<Self, UnknownValue> {
        Ok(Self {
            capture: Capture::of(settings),
            thread: Some(Captured {
                messages: session.message_count,
                updated_at: super::rfc3339(session.updated_at),
                updated: session.updated_at,
            }),
            octavo: Some(shares.map_or(Ok(Shares::OFF), Shares::try_from)?),
        })
    }

    pub(in crate::commands) fn print(&self, style: Style) -> Result<()> {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        if style.is_json() {
            serde_json::to_writer(&mut out, self)?;
            writeln!(out)?;
            return Ok(());
        }
        write_status(&mut out, self)?;
        Ok(())
    }
}

/// Whether thread capture is enabled in settings.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Capture {
    On,
    Off,
}

impl Capture {
    const fn of(settings: &Settings) -> Self {
        if settings.threads.enabled {
            Self::On
        } else {
            Self::Off
        }
    }
}

#[derive(Debug, Serialize)]
struct Captured {
    messages: u64,
    /// RFC 3339 timestamp of the newest captured message.
    updated_at: String,
    #[serde(skip)]
    updated: Timestamp,
}

/// Octavo sharing state for a thread.
#[derive(Debug, Serialize)]
struct Shares {
    upload: Upload,
    orgs: Vec<OrgShare>,
    dirs: Vec<DirShares>,
    private: bool,
    revoking_links: bool,
}

impl Shares {
    const OFF: Self = Self {
        upload: Upload::Off,
        orgs: Vec::new(),
        dirs: Vec::new(),
        private: false,
        revoking_links: false,
    };
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Upload {
    Off,
    LoggedOut,
    On,
}

#[derive(Debug, Clone, Copy, Serialize, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
enum ShareState {
    Opening,
    Open,
    Closing,
    Closed,
}

#[derive(Debug, Serialize)]
struct OrgShare {
    slug: String,
    state: ShareState,
}

#[derive(Debug, Serialize)]
struct DirShares {
    dir: String,
    orgs: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("the daemon sent a {field} this build doesn't know: {value}")]
pub(in crate::commands) struct UnknownValue {
    field: &'static str,
    value: i32,
}

impl TryFrom<pb::ThreadShares> for Shares {
    type Error = UnknownValue;

    fn try_from(shares: pb::ThreadShares) -> Result<Self, Self::Error> {
        let unknown = || UnknownValue {
            field: "upload state",
            value: shares.upload,
        };
        let upload = match pb::Upload::try_from(shares.upload).map_err(|_| unknown())? {
            pb::Upload::Off => Upload::Off,
            pb::Upload::LoggedOut => Upload::LoggedOut,
            pb::Upload::On => Upload::On,
            pb::Upload::Unspecified => return Err(unknown()),
        };
        Ok(Self {
            upload,
            orgs: shares.orgs.into_iter().map(OrgShare::try_from).collect::<Result<_, _>>()?,
            dirs: shares
                .dirs
                .into_iter()
                .map(|dir| DirShares {
                    dir: dir.dir,
                    orgs: dir.org_slugs,
                })
                .collect(),
            private: shares.is_private,
            revoking_links: shares.revoking_links,
        })
    }
}

impl TryFrom<pb::OrgShare> for OrgShare {
    type Error = UnknownValue;

    fn try_from(share: pb::OrgShare) -> Result<Self, Self::Error> {
        let unknown = || UnknownValue {
            field: "share state",
            value: share.state,
        };
        let state = match pb::ShareState::try_from(share.state).map_err(|_| unknown())? {
            pb::ShareState::Opening => ShareState::Opening,
            pb::ShareState::Open => ShareState::Open,
            pb::ShareState::Closing => ShareState::Closing,
            pb::ShareState::Closed => ShareState::Closed,
            pb::ShareState::Unspecified => return Err(unknown()),
        };
        Ok(Self {
            slug: share.org_slug,
            state,
        })
    }
}
