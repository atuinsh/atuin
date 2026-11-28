//! Thread capture, sync and Octavo sharing status for `atuin threads status`.

use std::io::{self, Write};

use atuin_client::ai_session::Session;
use atuin_client::settings::Settings;
use atuin_daemon::grpc::octavo::pb;
use atuin_daemon::{AiClient, ThreadsClient};
use eyre::{Result, eyre};
use jiff::Timestamp;
use serde::Serialize;
use strum_macros::IntoStaticStr;
use url::Url;

use super::Style;

/// Print capture, sync and Octavo sharing status for the thread matching `selector`.
pub(super) async fn status(
    settings: &Settings,
    client: &mut AiClient,
    selector: &str,
    style: Style,
) -> Result<()> {
    let Some(session) = super::find(client, selector).await? else {
        return Status::uncaptured(settings, selector).await?.print(style);
    };
    let shares = ThreadsClient::from_settings(settings)
        .await?
        .shares(session.sharing().clone())
        .await
        .map_err(|status| eyre!("{}", status.message()))?;
    Status::of(settings, &session, shares).await?.print(style)
}

fn write_status(out: &mut dyn Write, status: &Status) -> io::Result<()> {
    if matches!(status.capture, Capture::Off) {
        writeln!(
            out,
            "Thread capture is off. `atuin setup --only --threads --daemon` turns it on."
        )?;
    }
    let Some(thread) = &status.thread else {
        writeln!(out, "Not captured yet.")?;
        return status.octavo.as_ref().map_or(Ok(()), |shares| write_plain(out, shares));
    };
    let noun = if thread.messages == 1 {
        "message"
    } else {
        "messages"
    };
    let age = super::age(thread.updated);
    writeln!(out, "Captured: {} {noun}, the newest {age}.", thread.messages)?;
    write_sync(out, status.sync)?;
    status.octavo.as_ref().map_or(Ok(()), |shares| write_plain(out, shares))
}

fn write_sync(out: &mut dyn Write, sync: Syncing) -> io::Result<()> {
    match sync {
        Syncing::Off => writeln!(
            out,
            "Sync is off, so it reaches your other machines only when `atuin sync` runs."
        ),
        Syncing::LoggedOut => {
            writeln!(out, "Logged out, so it stays on this machine. Run `atuin login` to sync it.")
        }
        Syncing::On => writeln!(out, "Syncs to your other machines."),
    }
}

fn write_plain(out: &mut dyn Write, shares: &Shares) -> io::Result<()> {
    match shares.upload {
        // Without Atuin Hub there is nothing to say about sharing.
        Upload::Off => return Ok(()),
        Upload::LoggedOut => {
            return writeln!(
                out,
                "Logged out of Atuin Hub, so this machine shares no thread, and can't say who \
                 this one was shared with before. Run `atuin login`."
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

/// Capture, sync and Octavo sharing status for a thread; serialized directly for `--style json`.
#[derive(Debug, Serialize)]
pub(in crate::commands) struct Status {
    capture: Capture,
    sync: Syncing,
    /// `None` if the thread has not been captured yet.
    thread: Option<Captured>,
    /// Who a thread not captured yet will be shared with, if an agent started it lately, else
    /// `None` until it is captured.
    octavo: Option<Shares>,
}

impl Status {
    /// Status for `id`, a thread atuin has captured nothing of yet: who it will be shared with,
    /// if an agent started it lately.
    async fn uncaptured(settings: &Settings, id: &str) -> Result<Self> {
        let shares = ThreadsClient::from_settings(settings)
            .await?
            .new_thread_shares(id)
            .await
            .map_err(|status| eyre!("{}", status.message()))?;
        let hub = settings.hub_endpoint();
        Ok(Self {
            capture: Capture::of(settings),
            sync: Syncing::of(settings).await?,
            thread: None,
            octavo: shares.map(|shares| Shares::of(shares, &hub)).transpose()?,
        })
    }

    /// Build status for a captured `session`, treating `shares = None` as Octavo disabled.
    pub(in crate::commands) async fn of(
        settings: &Settings,
        session: &Session,
        shares: Option<pb::ThreadShares>,
    ) -> Result<Self> {
        Ok(Self {
            capture: Capture::of(settings),
            sync: Syncing::of(settings).await?,
            thread: Some(Captured {
                messages: session.message_count,
                updated_at: super::rfc3339(session.updated_at),
                updated: session.updated_at,
            }),
            octavo: Some(
                shares.map_or(Ok(Shares::OFF), |shares| {
                    Shares::of(shares, &settings.hub_endpoint())
                })?,
            ),
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

/// Whether atuin sync takes this machine's threads to the user's other machines.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Syncing {
    /// Logged in, but `auto_sync` or `daemon.sync_frequency` turns the daemon's sync off.
    Off,
    LoggedOut,
    On,
}

impl Syncing {
    /// As the daemon's sync worker decides whether to sync.
    async fn of(settings: &Settings) -> Result<Self> {
        if !settings.logged_in().await? {
            return Ok(Self::LoggedOut);
        }
        let is_periodic = settings.auto_sync && settings.daemon.sync_frequency.is_some();
        Ok(if is_periodic {
            Self::On
        } else {
            Self::Off
        })
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
    /// The organization's dashboard on Atuin Hub, if the Hub's address can hold one.
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
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

impl Shares {
    /// The daemon's `shares`, each organization's page on the Atuin Hub at `hub`.
    fn of(shares: pb::ThreadShares, hub: &Url) -> Result<Self, UnknownValue> {
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
            orgs: shares
                .orgs
                .into_iter()
                .map(|share| OrgShare::of(share, hub))
                .collect::<Result<_, _>>()?,
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

impl OrgShare {
    fn of(share: pb::OrgShare, hub: &Url) -> Result<Self, UnknownValue> {
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
        // Only a link is lost to an address that can't hold the page, so the status still shows.
        let url = hub.join(&share.org_slug).ok();
        Ok(Self {
            slug: share.org_slug,
            state,
            url,
        })
    }
}
