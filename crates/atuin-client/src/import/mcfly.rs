use std::process::Command;

use async_trait::async_trait;
use eyre::{Result, eyre};
use serde::Deserialize;
use time::OffsetDateTime;

use super::{Importer, Loader};
use crate::history::History;

#[derive(Debug, Deserialize)]
struct McflyEntry {
    when_run: String,
    cmd: String,
}

#[derive(Debug)]
pub struct Mcfly {
    entries: Vec<McflyEntry>,
}

fn parse_timestamp(s: &str) -> Result<OffsetDateTime> {
    // Try RFC3339 format first (e.g., "2023-01-01T12:00:00Z")
    if let Ok(ts) = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Ok(ts);
    }

    // Fall back to Unix timestamp (e.g., "1672574410")
    let unix_ts = s
        .parse::<i64>()
        .map_err(|_| eyre!("Failed to parse timestamp: {}", s))?;

    OffsetDateTime::from_unix_timestamp(unix_ts).map_err(|_| eyre!("Invalid Unix timestamp: {}", s))
}

#[async_trait]
impl Importer for Mcfly {
    const NAME: &'static str = "mcfly";

    async fn new() -> Result<Self> {
        // Check if mcfly is installed
        let output = Command::new("mcfly")
            .arg("dump")
            .output()
            .map_err(|_| eyre!("mcfly not found in PATH. Please ensure mcfly is installed"))?;

        if !output.status.success() {
            return Err(eyre!(
                "Failed to dump mcfly history: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let json_str = String::from_utf8(output.stdout)?;
        let entries: Vec<McflyEntry> = serde_json::from_str(&json_str)?;

        Ok(Self { entries })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(self.entries.len())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        for entry in self.entries {
            let timestamp = parse_timestamp(&entry.when_run)?;

            let imported = History::import().timestamp(timestamp).command(entry.cmd);

            h.push(imported.build().into()).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::{Mcfly, parse_timestamp};
    use crate::import::{Importer, tests::TestLoader};
    use time::macros::datetime;

    #[test]
    fn test_parse_timestamp() {
        // Test RFC3339 format
        let ts1 = parse_timestamp("2023-01-01T12:00:00Z").unwrap();
        assert_eq!(ts1, datetime!(2023-01-01 12:00:00 UTC));

        // Test Unix timestamp
        let ts2 = parse_timestamp("1672574400").unwrap();
        assert_eq!(ts2, datetime!(2023-01-01 12:00:00 UTC));

        // Test invalid timestamp
        assert!(parse_timestamp("invalid").is_err());
        assert!(parse_timestamp("999999999999999").is_err()); // Unix timestamp out of range
    }

    #[tokio::test]
    async fn parse_mcfly_history() {
        // Create a mock mcfly history with various timestamp formats
        let entries = vec![
            super::McflyEntry {
                when_run: "2023-01-01T12:00:00Z".to_string(),
                cmd: "ls -la".to_string(),
            },
            super::McflyEntry {
                when_run: "2023-01-01T12:00:05Z".to_string(),
                cmd: "cd /home".to_string(),
            },
            super::McflyEntry {
                when_run: "1672574410".to_string(), // Unix timestamp: 2023-01-01T12:00:10Z
                cmd: "echo hello".to_string(),
            },
        ];

        let mcfly = Mcfly { entries };
        let mut loader = TestLoader::default();
        mcfly.load(&mut loader).await.unwrap();

        // Verify count and that all entries were imported
        assert_eq!(loader.buf.len(), 3);

        // Verify timestamps are parsed correctly for both RFC3339 and Unix timestamp formats
        assert_eq!(loader.buf[0].timestamp, datetime!(2023-01-01 12:00:00 UTC));
        assert_eq!(loader.buf[1].timestamp, datetime!(2023-01-01 12:00:05 UTC));
        assert_eq!(loader.buf[2].timestamp, datetime!(2023-01-01 12:00:10 UTC));

        // Since mcfly doesn't transform commands, just verify they're imported as-is
        let commands: Vec<&str> = loader.buf.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(commands, vec!["ls -la", "cd /home", "echo hello"]);

        // Verify timestamps are in order
        assert!(
            loader
                .buf
                .windows(2)
                .all(|w| w[0].timestamp <= w[1].timestamp)
        );
    }

    #[tokio::test]
    async fn parse_mcfly_with_invalid_timestamp() {
        let entries = vec![
            super::McflyEntry {
                when_run: "2023-01-01T12:00:00Z".to_string(),
                cmd: "valid command".to_string(),
            },
            super::McflyEntry {
                when_run: "invalid_timestamp".to_string(),
                cmd: "command with bad timestamp".to_string(),
            },
        ];

        let mcfly = Mcfly { entries };
        let mut loader = TestLoader::default();

        // Should fail on invalid timestamp
        let result = mcfly.load(&mut loader).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse timestamp")
        );
    }

    #[tokio::test]
    async fn parse_empty_mcfly_history() {
        let entries = vec![];
        let mcfly = Mcfly { entries };
        let mut loader = TestLoader::default();

        // Should handle empty history gracefully
        mcfly.load(&mut loader).await.unwrap();
        assert_eq!(loader.buf.len(), 0);
    }
}
