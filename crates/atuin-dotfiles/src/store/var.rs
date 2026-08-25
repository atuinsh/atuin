/// Store for shell vars
/// I should abstract this and reuse code between the alias/env stores
/// This is easier for now
/// Once I have two implementations, building a common base is much easier.
use std::collections::BTreeMap;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{
    DecryptedData, Host, HostId, RecordSeriesKey, RecordTag, RecordVersion,
};
use eyre::{Result, bail, ensure, eyre};

use crate::shell::Var;

const DOTFILES_VAR_LEN: usize = 20000; // 20kb max total len, way more than should be needed.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarRecord {
    Create(Var),    // create a full record
    Delete(String), // delete by name
}

impl VarRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        match self {
            Self::Create(env) => {
                encode::write_u8(&mut output, 0)?; // create

                env.serialize(&mut output)?;
            }
            Self::Delete(env) => {
                encode::write_u8(&mut output, 1)?; // delete
                encode::write_array_len(&mut output, 1)?; // 1 field

                encode::write_str(&mut output, env.as_str())?;
            }
        }

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &RecordVersion) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        match version {
            RecordVersion::V0 => {
                let mut bytes = decode::Bytes::new(&data.0);

                let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

                match record_type {
                    // create
                    0 => {
                        let env = Var::deserialize(&mut bytes)?;
                        Ok(Self::Create(env))
                    }

                    // delete
                    1 => {
                        let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                        ensure!(nfields == 1, "too many entries in v0 dotfiles var delete record");

                        let bytes = bytes.remaining_slice();

                        let (key, bytes) =
                            decode::read_str_from_slice(bytes).map_err(error_report)?;

                        if !bytes.is_empty() {
                            bail!("trailing bytes in encoded dotfiles var record. malformed");
                        }

                        Ok(Self::Delete(key.to_owned()))
                    }

                    n => {
                        bail!("unknown Dotfiles var record type {n}");
                    }
                }
            }
            other => {
                bail!("unknown var record version {other:?}");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct VarStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: paseto_v4::Key,
}

impl VarStore {
    // will want to init the actual kv store when that is done
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: paseto_v4::Key) -> Self {
        Self {
            store,
            host_id,
            encryption_key,
        }
    }

    /// Escape a value for use in POSIX shells (bash, zsh)
    /// This adds double quotes around the value and escapes any embedded double quotes
    fn escape_posix_value(value: &str) -> String {
        // If the value contains no special characters, we can use it unquoted
        if value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '.')
        {
            value.to_string()
        } else {
            // Otherwise, wrap in double quotes and escape any special characters
            format!(
                "\"{}\"",
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('$', "\\$")
                    .replace('`', "\\`")
            )
        }
    }

    /// Escape a value for use in fish shell
    /// Fish uses single quotes for literal strings, but we need to handle embedded single quotes
    fn escape_fish_value(value: &str) -> String {
        // If the value contains no special characters, we can use it unquoted
        if value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '.')
        {
            value.to_string()
        } else {
            // Use single quotes and escape any embedded single quotes
            format!("'{}'", value.replace('\'', "\\'"))
        }
    }

    /// Escape a value for use in xonsh
    /// Xonsh uses Python-style string literals
    fn escape_xonsh_value(value: &str) -> String {
        // If the value contains no special characters, we can use it unquoted
        if value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '.')
        {
            value.to_string()
        } else {
            // Use double quotes and escape appropriately for Python strings
            format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
        }
    }

    pub async fn xonsh(&self) -> Result<String> {
        let env = self.vars().await?;
        Ok(Self::format_xonsh(&env))
    }

    pub async fn fish(&self) -> Result<String> {
        let env = self.vars().await?;
        Ok(Self::format_fish(&env))
    }

    pub async fn posix(&self) -> Result<String> {
        let env = self.vars().await?;
        Ok(Self::format_posix(&env))
    }

    pub async fn powershell(&self) -> Result<String> {
        let env = self.vars().await?;
        Ok(Self::format_powershell(&env))
    }

    fn format_xonsh(env: &[Var]) -> String {
        let mut config = String::new();

        for env in env {
            let escaped_value = Self::escape_xonsh_value(&env.value);
            config.push_str(&format!("${}={}\n", env.name, escaped_value));
        }

        config
    }

    fn format_fish(env: &[Var]) -> String {
        let mut config = String::new();

        for env in env {
            let escaped_value = Self::escape_fish_value(&env.value);
            config.push_str(&format!("set -gx {} {}\n", env.name, escaped_value));
        }

        config
    }

    fn format_posix(env: &[Var]) -> String {
        let mut config = String::new();

        for env in env {
            let escaped_value = Self::escape_posix_value(&env.value);
            if env.export {
                config.push_str(&format!("export {}={}\n", env.name, escaped_value));
            } else {
                config.push_str(&format!("{}={}\n", env.name, escaped_value));
            }
        }

        config
    }

    fn format_powershell(env: &[Var]) -> String {
        let mut config = String::new();

        for var in env {
            config.push_str(&crate::shell::powershell::format_var(var));
        }

        config
    }

    pub async fn build(&self) -> Result<()> {
        let dir = atuin_common::utils::dotfiles_cache_dir();
        tokio::fs::create_dir_all(dir.clone()).await?;

        let env = self.vars().await?;

        // Build for all supported shells
        let posix = Self::format_posix(&env);
        let xonsh = Self::format_xonsh(&env);
        let fsh = Self::format_fish(&env);
        let powershell = Self::format_powershell(&env);

        // All the same contents, maybe optimize in the future or perhaps there will be quirks
        // per-shell
        // I'd prefer separation atm
        let zsh = dir.join("vars.zsh");
        let bash = dir.join("vars.bash");
        let fish = dir.join("vars.fish");
        let xsh = dir.join("vars.xsh");
        let ps1 = dir.join("vars.ps1");

        tokio::fs::write(zsh, &posix).await?;
        tokio::fs::write(bash, &posix).await?;
        tokio::fs::write(fish, &fsh).await?;
        tokio::fs::write(xsh, &xonsh).await?;
        tokio::fs::write(ps1, &powershell).await?;

        Ok(())
    }

    pub async fn set(&self, name: &str, value: &str, export: bool) -> Result<()> {
        if name.len() + value.len() > DOTFILES_VAR_LEN {
            return Err(eyre!("var record too large: max len {} bytes", DOTFILES_VAR_LEN));
        }

        let record = VarRecord::Create(Var {
            name: name.to_string(),
            value: value.to_string(),
            export,
        });

        let bytes = record.serialize()?;

        let idx = self
            .store
            .last(&RecordSeriesKey::new(self.host_id, RecordTag::DotfilesVar))
            .await?
            .map_or(0, |entry| entry.idx + 1);

        let record = atuin_domain::record::Record::builder()
            .host(Host::new(self.host_id))
            .version(RecordVersion::V0)
            .tag(RecordTag::DotfilesVar)
            .idx(idx)
            .data(bytes)
            .build();

        self.store.push(&record.encrypt(&self.encryption_key)).await?;

        // set mutates shell config, so build again
        self.build().await?;

        Ok(())
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        if name.len() > DOTFILES_VAR_LEN {
            return Err(eyre!("var record too large: max len {} bytes", DOTFILES_VAR_LEN,));
        }

        let record = VarRecord::Delete(name.to_string());

        let bytes = record.serialize()?;

        let idx = self
            .store
            .last(&RecordSeriesKey::new(self.host_id, RecordTag::DotfilesVar))
            .await?
            .map_or(0, |entry| entry.idx + 1);

        let record = atuin_domain::record::Record::builder()
            .host(Host::new(self.host_id))
            .version(RecordVersion::V0)
            .tag(RecordTag::DotfilesVar)
            .idx(idx)
            .data(bytes)
            .build();

        self.store.push(&record.encrypt(&self.encryption_key)).await?;

        // delete mutates shell config, so build again
        self.build().await?;

        Ok(())
    }

    pub async fn vars(&self) -> Result<Vec<Var>> {
        let mut build = BTreeMap::new();

        // this is sorted, oldest to newest
        let tagged = self.store.all_tagged(&RecordTag::DotfilesVar).await?;
        let mut skipped = 0;

        for record in tagged {
            let version = record.version.clone();

            // Skip records we can't decrypt or decode, rather than failing the entire build.
            let ar = match version {
                RecordVersion::V0 => record.decrypt(&self.encryption_key).and_then(|decrypted| {
                    VarRecord::deserialize(&decrypted.data, &RecordVersion::V0)
                }),
                ref version => Err(eyre!("unknown version {version:?}")),
            };

            let ar = match ar {
                Ok(ar) => ar,
                Err(e) => {
                    tracing::warn!("failed to decode var record, skipping: {e}");
                    skipped += 1;
                    continue;
                }
            };

            match ar {
                VarRecord::Create(a) => {
                    build.insert(a.name.clone(), a);
                }
                VarRecord::Delete(d) => {
                    build.remove(&d);
                }
            }
        }

        if skipped > 0 {
            // vars() runs during shell init, so this must not write to stderr
            tracing::warn!("skipped {skipped} var records that could not be decrypted or decoded");
        }

        Ok(build.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_domain::record::RecordVersion;
    use crypto_secretbox::{KeyInit, XSalsa20Poly1305};
    use rand::rngs::OsRng;
    use rstest::*;

    use super::{VarRecord, VarStore};
    use crate::shell::Var;
    use crate::store::test_local_timeout;

    #[fixture]
    async fn var_store() -> VarStore {
        let store = SqliteStore::new(":memory:", test_local_timeout()).await.unwrap();
        let key: [u8; 32] = XSalsa20Poly1305::generate_key(&mut OsRng).into();
        let host_id = atuin_domain::record::HostId(atuin_common::utils::uuid_v7());

        VarStore::new(store, host_id, key.into())
    }

    #[rstest]
    fn encode_decode() {
        let record = Var {
            name: "BEEP".to_owned(),
            value: "boop".to_owned(),
            export: false,
        };
        let record = VarRecord::Create(record);

        let snapshot = [204, 0, 147, 164, 66, 69, 69, 80, 164, 98, 111, 111, 112, 194];

        let encoded = record.serialize().unwrap();
        let decoded = VarRecord::deserialize(&encoded, &RecordVersion::V0).unwrap();

        assert_eq!(encoded.0, &snapshot);
        assert_eq!(decoded, record);
    }

    #[rstest]
    // Simple values should not be quoted
    #[case::simple("simple", "simple")]
    #[case::path("path/to/file", "path/to/file")]
    #[case::underscores("value_with_underscores", "value_with_underscores")]
    // Values with spaces should be quoted
    #[case::spaces("hello world", "\"hello world\"")]
    #[case::spaces_short("bar baz", "\"bar baz\"")]
    // Values with special characters should be quoted and escaped
    #[case::double_quotes("say \"hello\"", "\"say \\\"hello\\\"\"")]
    #[case::backslashes("path\\with\\backslashes", "\"path\\\\with\\\\backslashes\"")]
    #[case::dollar("say $hello", "\"say \\$hello\"")]
    #[case::backticks("see `example.md`", "\"see \\`example.md\\`\"")]
    fn escapes_posix_value(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(VarStore::escape_posix_value(input), expected);
    }

    #[rstest]
    // Simple values should not be quoted
    #[case::simple("simple", "simple")]
    #[case::path("path/to/file", "path/to/file")]
    // Values with spaces should be single-quoted
    #[case::spaces("hello world", "'hello world'")]
    #[case::spaces_short("bar baz", "'bar baz'")]
    // Values with single quotes should be escaped
    #[case::single_quote("don't", "'don\\'t'")]
    fn escapes_fish_value(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(VarStore::escape_fish_value(input), expected);
    }

    #[rstest]
    // Simple values should not be quoted
    #[case::simple("simple", "simple")]
    #[case::path("path/to/file", "path/to/file")]
    // Values with spaces should be quoted
    #[case::spaces("hello world", "\"hello world\"")]
    #[case::spaces_short("bar baz", "\"bar baz\"")]
    // Values with special characters should be quoted and escaped
    #[case::double_quotes("say \"hello\"", "\"say \\\"hello\\\"\"")]
    #[case::backslashes("path\\with\\backslashes", "\"path\\\\with\\\\backslashes\"")]
    fn escapes_xonsh_value(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(VarStore::escape_xonsh_value(input), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn build_vars(#[future] var_store: VarStore) {
        let env = var_store.await;

        env.set("BEEP", "boop", false).await.unwrap();
        env.set("HOMEBREW_NO_AUTO_UPDATE", "1", true).await.unwrap();

        let mut env_vars = env.vars().await.unwrap();

        env_vars.sort_by_key(|a| a.name.clone());

        assert_eq!(env_vars.len(), 2);

        assert_eq!(env_vars[0], Var {
            name: String::from("BEEP"),
            value: String::from("boop"),
            export: false,
        });

        assert_eq!(env_vars[1], Var {
            name: String::from("HOMEBREW_NO_AUTO_UPDATE"),
            value: String::from("1"),
            export: true,
        });
    }

    #[rstest]
    #[tokio::test]
    async fn test_var_generation_with_spaces(#[future] var_store: VarStore) {
        let env = var_store.await;

        // Test the exact scenario from the bug report
        env.set("FOO", "bar baz", true).await.unwrap();

        let posix_output = env.posix().await.unwrap();
        let fish_output = env.fish().await.unwrap();
        let xonsh_output = env.xonsh().await.unwrap();

        // POSIX should quote the value
        assert_eq!(posix_output, "export FOO=\"bar baz\"\n");

        // Fish should quote the value
        assert_eq!(fish_output, "set -gx FOO 'bar baz'\n");

        // Xonsh should quote the value
        assert_eq!(xonsh_output, "$FOO=\"bar baz\"\n");
    }
}
