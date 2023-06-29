use atuin_common::record::RecordIndex;
// do a sync :O
use eyre::Result;
use uuid::Uuid;

use crate::{api_client::Client, settings::Settings};

use super::store::Store;

pub async fn diff(
    settings: &Settings,
    store: &mut impl Store,
) -> Result<Vec<(Uuid, String, Uuid)>> {
    let client = Client::new(&settings.sync_address, &settings.session_token)?;

    // First, build our own index
    let local_tail = store.tail_records().await?;
    let local_index = RecordIndex::from(local_tail);

    let remote_index = client.record_index().await?;

    let diff = local_index.diff(&remote_index);

    Ok(diff)
}
