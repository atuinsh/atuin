use std::convert::TryInto;

use chrono::prelude::*;
use eyre::Result;

use crate::local::api_client;
use crate::local::database::Database;
use crate::local::encryption::{encrypt, load_key};
use crate::settings::{Local, Settings, HISTORY_PAGE_SIZE};
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
    force: bool,
    client: &api_client::Client,
    db: &mut impl Database,
) -> Result<(i64, i64)> {
    let remote_count = client.count()?;

    let initial_local = db.history_count()?;
    let mut local_count = initial_local;

    let mut last_sync = if force {
        Utc.timestamp_millis(0)
    } else {
        Local::last_sync()?
    };

    let mut last_timestamp = Utc.timestamp_millis(0);

    let host = if force { Some(String::from("")) } else { None };

    while remote_count > local_count {
        let page = client.get_history(last_sync, last_timestamp, host.clone())?;

        if page.len() < HISTORY_PAGE_SIZE.try_into().unwrap() {
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
    _force: bool,
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
        let last = db.before(cursor, HISTORY_PAGE_SIZE)?;
        let mut buffer = Vec::<AddHistoryRequest>::new();

        if last.is_empty() {
            break;
        }

        for i in last {
            let data = encrypt(&i, &key)?;
            let data = serde_json::to_string(&data)?;

            let add_hist = AddHistoryRequest {
                id: i.id,
                timestamp: i.timestamp,
                data,
                hostname: hash_str(i.hostname.as_str()),
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

pub fn sync(settings: &Settings, force: bool, db: &mut impl Database) -> Result<()> {
    let client = api_client::Client::new(settings);

    sync_upload(settings, force, &client, db)?;

    let download = sync_download(force, &client, db)?;

    debug!("sync downloaded {}", download.0);

    Local::save_sync_time()?;

    Ok(())
}
