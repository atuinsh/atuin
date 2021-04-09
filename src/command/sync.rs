use std::collections::HashMap;

use base64;
use diesel::expression::ops::Add;
use eyre::{eyre, Result};
use reqwest::{blocking::Response, header::AUTHORIZATION};

use crate::local::database::Database;
use crate::local::encryption::{encrypt, load_key};
use crate::local::history::History;
use crate::settings::Settings;

#[derive(Serialize, Deserialize)]
struct AddHistory {
    id: String,
    timestamp: i64,
    data: String,
}

pub fn run(settings: &Settings, db: &mut impl Database) -> Result<()> {
    let mut buffer = Vec::<AddHistory>::new();

    let sync_buffer = |b: &[AddHistory]| -> Result<Response> {
        let token = std::fs::read_to_string(settings.local.session_path.as_str())?;

        let url = format!("{}/history", settings.local.sync_address);
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(url)
            .json(b)
            .header(AUTHORIZATION, format!("Token {}", token))
            .send()?;

        Ok(resp)
    };

    for i in db.list()? {
        let key = load_key(settings)?;
        let data = encrypt(settings, &i, &key)?;

        let add_hist = AddHistory {
            id: i.id,
            timestamp: i.timestamp,
            data: base64::encode(data.ciphertext),
        };

        buffer.push(add_hist);

        if buffer.len() >= 100 {
            sync_buffer(&buffer)?;
            buffer = Vec::new();
        }
    }

    Ok(())
}
