use chrono::prelude::*;
use eyre::Result;
use reqwest::{blocking::Response, header::AUTHORIZATION};

use crate::local::api_client;
use crate::local::database::Database;
use crate::local::encryption::{encrypt, load_key};
use crate::settings::Settings;
use crate::{api::AddHistoryRequest, utils::hash_str};

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
fn sync_download(
    settings: &Settings,
    client: &api_client::Client,
    db: &mut impl Database,
) -> Result<(i64, i64)> {
    let remote_count = client.count()?;

    let initial_local = db.history_count()?;
    let mut local_count = initial_local;

    let mut last_sync = settings.local.last_sync()?;
    let mut last_timestamp = Utc.timestamp_millis(0);

    while remote_count > local_count {
        let page = client.get_history(last_sync, last_timestamp)?;

        if page.len() == 0 {
            break;
        }

        db.save_bulk(&page)?;

        local_count = db.history_count()?;

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
fn sync_upload(
    settings: &Settings,
    client: &api_client::Client,
    db: &mut impl Database,
) -> Result<()> {
    let initial_remote_count = client.count()?;
    let mut remote_count = initial_remote_count;

    let local_count = db.history_count()?;

    let key = load_key(settings)?; // encryption key

    // first just try the most recent set

    let mut cursor = Utc::now();

    while local_count > remote_count {
        let last = db.before(cursor, 100)?;
        let mut buffer = Vec::<AddHistoryRequest>::new();

        if last.len() == 0 {
            break;
        }

        for i in last {
            let data = encrypt(settings, &i, &key)?;
            let data = serde_json::to_string(&data)?;

            let add_hist = AddHistoryRequest {
                id: i.id,
                timestamp: i.timestamp,
                data,
                hostname: hash_str(hostname::get()?.to_str().unwrap()),
            };

            buffer.push(add_hist);
        }

        // anything left over outside of the 100 block size
        client.post_history(&buffer)?;
        cursor = buffer.last().unwrap().timestamp;

        remote_count = client.count()?;
    }

    Ok(())
}

pub fn sync(settings: &Settings, db: &mut impl Database) -> Result<()> {
    let client = api_client::Client::new(settings);

    sync_upload(settings, &client, db)?;

    let download = sync_download(settings, &client, db)?;

    debug!("sync downloaded {}", download.0);

    settings.local.save_sync_time()?;

    Ok(())
}
