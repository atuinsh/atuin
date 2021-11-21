use std::convert::TryInto;

use chrono::prelude::*;
use eyre::{eyre, Result};

use atuin_common::{api::AddHistoryRequest, utils::hash_str};

use crate::api_client;
use crate::database::Database;
use crate::encryption::{encrypt, load_encoded_key, load_key};
use crate::settings::{Settings, HISTORY_PAGE_SIZE};

// Currently sync is kinda naive, and basically just pages backwards through
// history. This means newly added stuff shows up properly! We also just use
// the total count in each database to indicate whether a sync is needed.
// I think this could be massively improved! If we had a way of easily
// indicating count per time period (hour, day, week, year, etc) then we can
// easily pinpoint where we are missing data and what needs downloading. Start
// with year, then find the week, then the day, then the hour, then download it
// all! The current naive approach will do for now.

// Check if remote has things we don't, and if so, download them.
// Returns (num downloaded, total local)
async fn sync_download(
    force: bool,
    client: &api_client::Client<'_>,
    db: &mut (impl Database + Send),
) -> Result<(i64, i64)> {
    debug!("starting sync download");

    let remote_count = client.count().await?;

    let initial_local = db.history_count().await?;
    let mut local_count = initial_local;

    let mut last_sync = if force {
        Utc.timestamp_millis(0)
    } else {
        Settings::last_sync()?
    };

    let mut last_timestamp = Utc.timestamp_millis(0);

    let host = if force { Some(String::from("")) } else { None };

    while remote_count > local_count {
        let page = client
            .get_history(last_sync, last_timestamp, host.clone())
            .await?;

        db.save_bulk(&page).await?;

        local_count = db.history_count().await?;

        if page.len() < HISTORY_PAGE_SIZE.try_into().unwrap() {
            break;
        }

        let page_last = page
            .last()
            .expect("could not get last element of page")
            .timestamp;

        // in the case of a small sync frequency, it's possible for history to
        // be "lost" between syncs. In this case we need to rewind the sync
        // timestamps
        if page_last == last_timestamp {
            last_timestamp = Utc.timestamp_millis(0);
            last_sync = last_sync - chrono::Duration::hours(1);
        } else {
            last_timestamp = page_last;
        }
    }

    Ok((local_count - initial_local, local_count))
}

// Check if we have things remote doesn't, and if so, upload them
async fn sync_upload(
    settings: &Settings,
    _force: bool,
    client: &api_client::Client<'_>,
    db: &mut (impl Database + Send),
) -> Result<()> {
    debug!("starting sync upload");

    let initial_remote_count = client.count().await?;
    let mut remote_count = initial_remote_count;

    let local_count = db.history_count().await?;

    debug!("remote has {}, we have {}", remote_count, local_count);

    let key = load_key(settings)?; // encryption key

    // first just try the most recent set

    let mut cursor = Utc::now();

    while local_count > remote_count {
        let last = db.before(cursor, HISTORY_PAGE_SIZE).await?;
        let mut buffer = Vec::new();

        if last.is_empty() {
            break;
        }

        for i in last {
            let data = encrypt(&i, &key)?;
            let data = serde_json::to_string(&data)?;

            let add_hist = AddHistoryRequest {
                id: i.id.into(),
                timestamp: i.timestamp,
                data,
                hostname: hash_str(&i.hostname).into(),
            };

            buffer.push(add_hist);
        }

        // anything left over outside of the 100 block size
        client.post_history(&buffer).await?;
        cursor = buffer.last().unwrap().timestamp;
        remote_count = client.count().await?;

        debug!("upload cursor: {:?}", cursor);
    }

    Ok(())
}

pub async fn sync(settings: &Settings, force: bool, db: &mut (impl Database + Send)) -> Result<()> {
    let client = api_client::Client::new(
        &settings.sync_address,
        settings.session_token.as_ref().ok_or_else(|| eyre!("not logged in"))?,
        load_encoded_key(settings)?,
    )?;

    sync_upload(settings, force, &client, db).await?;

    let download = sync_download(force, &client, db).await?;

    debug!("sync downloaded {}", download.0);

    Settings::save_sync_time()?;

    Ok(())
}
