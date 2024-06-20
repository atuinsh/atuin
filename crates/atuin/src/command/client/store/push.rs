use atuin_common::record::HostId;
use clap::Args;
use eyre::Result;
use uuid::Uuid;

use atuin_client::{
    api_client::Client,
    record::sync::Operation,
    record::{sqlite_store::SqliteStore, sync},
    settings::Settings,
};

#[derive(Args, Debug)]
pub struct Push {
    /// The tag to push (eg, 'history'). Defaults to all tags
    #[arg(long, short)]
    pub tag: Option<String>,

    /// The host to push, in the form of a UUID host ID. Defaults to the current host.
    #[arg(long)]
    pub host: Option<Uuid>,

    /// Force push records
    /// This will override both host and tag, to be all hosts and all tags. First clear the remote store, then upload all of the
    /// local store
    #[arg(long, default_value = "false")]
    pub force: bool,
}

impl Push {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let host_id = Settings::host_id().expect("failed to get host_id");

        if self.force {
            println!("Forcing remote store overwrite!");
            println!("Clearing remote store");

            let client = Client::new(
                &settings.sync_address,
                settings.session_token()?.as_str(),
                settings.network_connect_timeout,
                settings.network_timeout * 10, // we may be deleting a lot of data... so up the
                                               // timeout
            )
            .expect("failed to create client");

            client.delete_store().await?;
        }

        // We can actually just use the existing diff/etc to push
        // 1. Diff
        // 2. Get operations
        // 3. Filter operations by
        //  a) are they an upload op?
        //  b) are they for the host/tag we are pushing here?
        let (diff, _) = sync::diff(settings, &store).await?;
        let operations = sync::operations(diff, &store).await?;

        let operations = operations
            .into_iter()
            .filter(|op| match op {
                // No noops or downloads thx
                Operation::Noop { .. } | Operation::Download { .. } => false,

                // push, so yes plz to uploads!
                Operation::Upload { host, tag, .. } => {
                    if self.force {
                        return true;
                    }

                    if let Some(h) = self.host {
                        if HostId(h) != *host {
                            return false;
                        }
                    } else if *host != host_id {
                        return false;
                    }

                    if let Some(t) = self.tag.clone() {
                        if t != *tag {
                            return false;
                        }
                    }

                    true
                }
            })
            .collect();

        let (uploaded, _) = sync::sync_remote(operations, &store, settings).await?;

        println!("Uploaded {uploaded} records");

        Ok(())
    }
}
