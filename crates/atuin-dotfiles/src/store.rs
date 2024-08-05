use std::collections::BTreeMap;

use atuin_client::record::sqlite_store::SqliteStore;
// Sync aliases
// This will be noticeable similar to the kv store, though I expect the two shall diverge
// While we will support a range of shell config, I'd rather have a larger number of small records
// + stores, rather than one mega config store.
use atuin_common::record::{DecryptedData, Host, HostId};
use atuin_common::utils::unquote;
use eyre::{bail, ensure, eyre, Result};

use atuin_client::record::encryption::PASETO_V4;
use atuin_client::record::store::Store;

use crate::shell::Alias;

const CONFIG_SHELL_ALIAS_VERSION: &str = "v0";
const CONFIG_SHELL_ALIAS_TAG: &str = "config-shell-alias";
const CONFIG_SHELL_ALIAS_FIELD_MAX_LEN: usize = 20000; // 20kb max total len, way more than should be needed.

mod alias;
pub mod var;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasRecord {
    Create(Alias),  // create a full record
    Delete(String), // delete by name
}

impl AliasRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        match self {
            AliasRecord::Create(alias) => {
                encode::write_u8(&mut output, 0)?; // create
                encode::write_array_len(&mut output, 2)?; // 2 fields

                encode::write_str(&mut output, alias.name.as_str())?;
                encode::write_str(&mut output, alias.value.as_str())?;
            }
            AliasRecord::Delete(name) => {
                encode::write_u8(&mut output, 1)?; // delete
                encode::write_array_len(&mut output, 1)?; // 1 field

                encode::write_str(&mut output, name.as_str())?;
            }
        }

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        match version {
            CONFIG_SHELL_ALIAS_VERSION => {
                let mut bytes = decode::Bytes::new(&data.0);

                let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

                match record_type {
                    // create
                    0 => {
                        let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                        ensure!(
                            nfields == 2,
                            "too many entries in v0 shell alias create record"
                        );

                        let bytes = bytes.remaining_slice();

                        let (key, bytes) =
                            decode::read_str_from_slice(bytes).map_err(error_report)?;
                        let (value, bytes) =
                            decode::read_str_from_slice(bytes).map_err(error_report)?;

                        if !bytes.is_empty() {
                            bail!("trailing bytes in encoded shell alias record. malformed")
                        }

                        Ok(AliasRecord::Create(Alias {
                            name: key.to_owned(),
                            value: value.to_owned(),
                        }))
                    }

                    // delete
                    1 => {
                        let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                        ensure!(
                            nfields == 1,
                            "too many entries in v0 shell alias delete record"
                        );

                        let bytes = bytes.remaining_slice();

                        let (key, bytes) =
                            decode::read_str_from_slice(bytes).map_err(error_report)?;

                        if !bytes.is_empty() {
                            bail!("trailing bytes in encoded shell alias record. malformed")
                        }

                        Ok(AliasRecord::Delete(key.to_owned()))
                    }

                    n => {
                        bail!("unknown AliasRecord type {n}")
                    }
                }
            }
            _ => {
                bail!("unknown version {version:?}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct AliasStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl AliasStore {
    // will want to init the actual kv store when that is done
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> AliasStore {
        AliasStore {
            store,
            host_id,
            encryption_key,
        }
    }

    pub async fn posix(&self) -> Result<String> {
        let aliases = self.aliases().await?;

        let mut config = String::new();

        for alias in aliases {
            // If it's quoted, remove the quotes. If it's not quoted, do nothing.
            let value = unquote(alias.value.as_str()).unwrap_or(alias.value.clone());

            // we're about to quote it ourselves anyway!
            config.push_str(&format!("alias {}='{}'\n", alias.name, value));
        }

        Ok(config)
    }

    pub async fn xonsh(&self) -> Result<String> {
        let aliases = self.aliases().await?;

        let mut config = String::new();

        for alias in aliases {
            config.push_str(&format!("aliases['{}'] ='{}'\n", alias.name, alias.value));
        }

        Ok(config)
    }

    pub async fn build(&self) -> Result<()> {
        let dir = atuin_common::utils::dotfiles_cache_dir();
        tokio::fs::create_dir_all(dir.clone()).await?;

        // Build for all supported shells
        let posix = self.posix().await?;
        let xonsh = self.xonsh().await?;

        // All the same contents, maybe optimize in the future or perhaps there will be quirks
        // per-shell
        // I'd prefer separation atm
        let zsh = dir.join("aliases.zsh");
        let bash = dir.join("aliases.bash");
        let fish = dir.join("aliases.fish");
        let xsh = dir.join("aliases.xsh");

        tokio::fs::write(zsh, &posix).await?;
        tokio::fs::write(bash, &posix).await?;
        tokio::fs::write(fish, &posix).await?;
        tokio::fs::write(xsh, &xonsh).await?;

        Ok(())
    }

    pub async fn set(&self, name: &str, value: &str) -> Result<()> {
        if name.len() + value.len() > CONFIG_SHELL_ALIAS_FIELD_MAX_LEN {
            return Err(eyre!(
                "alias record too large: max len {} bytes",
                CONFIG_SHELL_ALIAS_FIELD_MAX_LEN
            ));
        }

        let record = AliasRecord::Create(Alias {
            name: name.to_string(),
            value: value.to_string(),
        });

        let bytes = record.serialize()?;

        let idx = self
            .store
            .last(self.host_id, CONFIG_SHELL_ALIAS_TAG)
            .await?
            .map_or(0, |entry| entry.idx + 1);

        let record = atuin_common::record::Record::builder()
            .host(Host::new(self.host_id))
            .version(CONFIG_SHELL_ALIAS_VERSION.to_string())
            .tag(CONFIG_SHELL_ALIAS_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        // set mutates shell config, so build again
        self.build().await?;

        Ok(())
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        if name.len() > CONFIG_SHELL_ALIAS_FIELD_MAX_LEN {
            return Err(eyre!(
                "alias record too large: max len {} bytes",
                CONFIG_SHELL_ALIAS_FIELD_MAX_LEN
            ));
        }

        let record = AliasRecord::Delete(name.to_string());

        let bytes = record.serialize()?;

        let idx = self
            .store
            .last(self.host_id, CONFIG_SHELL_ALIAS_TAG)
            .await?
            .map_or(0, |entry| entry.idx + 1);

        let record = atuin_common::record::Record::builder()
            .host(Host::new(self.host_id))
            .version(CONFIG_SHELL_ALIAS_VERSION.to_string())
            .tag(CONFIG_SHELL_ALIAS_TAG.to_string())
            .idx(idx)
            .data(bytes)
            .build();

        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;

        // delete mutates shell config, so build again
        self.build().await?;

        Ok(())
    }

    pub async fn aliases(&self) -> Result<Vec<Alias>> {
        let mut build = BTreeMap::new();

        // this is sorted, oldest to newest
        let tagged = self.store.all_tagged(CONFIG_SHELL_ALIAS_TAG).await?;

        for record in tagged {
            let version = record.version.clone();

            let decrypted = match version.as_str() {
                CONFIG_SHELL_ALIAS_VERSION => record.decrypt::<PASETO_V4>(&self.encryption_key)?,
                version => bail!("unknown version {version:?}"),
            };

            let ar = AliasRecord::deserialize(&decrypted.data, version.as_str())?;

            match ar {
                AliasRecord::Create(a) => {
                    build.insert(a.name.clone(), a);
                }
                AliasRecord::Delete(d) => {
                    build.remove(&d);
                }
            }
        }

        Ok(build.into_values().collect())
    }
}

#[cfg(test)]
pub(crate) fn test_local_timeout() -> f64 {
    std::env::var("ATUIN_TEST_LOCAL_TIMEOUT")
        .ok()
        .and_then(|x| x.parse().ok())
        // this hardcoded value should be replaced by a simple way to get the
        // default local_timeout of Settings if possible
        .unwrap_or(2.0)
}

#[cfg(test)]
mod tests {
    use rand::rngs::OsRng;

    use atuin_client::record::sqlite_store::SqliteStore;

    use crate::shell::Alias;

    use super::{test_local_timeout, AliasRecord, AliasStore, CONFIG_SHELL_ALIAS_VERSION};
    use crypto_secretbox::{KeyInit, XSalsa20Poly1305};

    #[test]
    fn encode_decode() {
        let record = Alias {
            name: "k".to_owned(),
            value: "kubectl".to_owned(),
        };
        let record = AliasRecord::Create(record);

        let snapshot = [204, 0, 146, 161, 107, 167, 107, 117, 98, 101, 99, 116, 108];

        let encoded = record.serialize().unwrap();
        let decoded = AliasRecord::deserialize(&encoded, CONFIG_SHELL_ALIAS_VERSION).unwrap();

        assert_eq!(encoded.0, &snapshot);
        assert_eq!(decoded, record);
    }

    #[tokio::test]
    async fn build_aliases() {
        let store = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let key: [u8; 32] = XSalsa20Poly1305::generate_key(&mut OsRng).into();
        let host_id = atuin_common::record::HostId(atuin_common::utils::uuid_v7());

        let alias = AliasStore::new(store, host_id, key);

        alias.set("k", "kubectl").await.unwrap();
        alias.set("gp", "git push").await.unwrap();
        alias
            .set("kgap", "'kubectl get pods --all-namespaces'")
            .await
            .unwrap();

        let mut aliases = alias.aliases().await.unwrap();

        aliases.sort_by_key(|a| a.name.clone());

        assert_eq!(aliases.len(), 3);

        assert_eq!(
            aliases[0],
            Alias {
                name: String::from("gp"),
                value: String::from("git push")
            }
        );

        assert_eq!(
            aliases[1],
            Alias {
                name: String::from("k"),
                value: String::from("kubectl")
            }
        );

        assert_eq!(
            aliases[2],
            Alias {
                name: String::from("kgap"),
                value: String::from("'kubectl get pods --all-namespaces'")
            }
        );

        let build = alias.posix().await.expect("failed to build aliases");

        assert_eq!(
            build,
            "alias gp='git push'
alias k='kubectl'
alias kgap='kubectl get pods --all-namespaces'
"
        )
    }
}
