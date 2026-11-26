//! `atuin hub`: who can open a thread on Atuin Hub.

use atuin_client::settings::Settings;
use atuin_daemon::grpc::octavo::pb;
use atuin_daemon::{AiClient, ThreadsClient};
use clap::{Args, Subcommand};
use eyre::{Result, eyre};

use super::session::octavo::Status;
use super::session::{self, Output};

/// `atuin hub`.
#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Stop sharing a thread: close it to every organization, revoke its links, and keep it from
    /// being shared again.
    Unshare(UnshareCmd),
}

impl Cmd {
    pub async fn run(self, settings: Settings) -> Result<()> {
        let mut client = AiClient::from_settings(&settings).await?;
        session::wait_for_sessions(&mut client).await?;
        match self {
            Self::Unshare(unshare) => unshare.run(&settings, &mut client).await,
        }
    }
}

#[derive(Args, Debug)]
pub struct UnshareCmd {
    #[arg(value_name = "ID|latest")]
    thread: String,

    #[command(flatten)]
    output: Output,
}

impl UnshareCmd {
    async fn run(self, settings: &Settings, client: &mut AiClient) -> Result<()> {
        let Some(session) = session::find(client, &self.thread).await? else {
            return Err(eyre!("no thread with id `{}`. Run `atuin threads list`.", self.thread));
        };
        let shares = ThreadsClient::from_settings(settings)
            .await?
            .make_private(session.sharing().clone())
            .await
            .map_err(|status| eyre!("{}", status.message()))?;
        match shares.as_ref().map(pb::ThreadShares::upload) {
            // A daemon built without Atuin Hub answers nothing.
            None | Some(pb::Upload::Off) => Err(eyre!(
                "Atuin Hub is off, so `atuin hub unshare` changed nothing. `atuin setup --only \
                 --hub` turns it on."
            )),
            Some(pb::Upload::LoggedOut) => Err(eyre!(
                "logged out of Atuin Hub, so `atuin hub unshare` changed nothing. Run `atuin \
                 login`."
            )),
            Some(pb::Upload::On | pb::Upload::Unspecified) => {
                Status::of(settings, &session, shares)?.print(self.output.style())
            }
        }
    }
}
