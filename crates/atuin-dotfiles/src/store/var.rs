/// Store for shell vars
/// I should abstract this and reuse code between the alias/env stores
/// This is easier for now
/// Once I have two implementations, building a common base is much easier.
use std::collections::BTreeMap;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::record::{DecryptedData, Host, HostId};
use eyre::{bail, ensure, eyre, Result};

use atuin_client::record::encryption::PASETO_V4;
use atuin_client::record::store::Store;

use crate::shell::Var;

const DOTFILES_VAR_VERSION: &str = "v0";
const DOTFILES_VAR_TAG: &str = "dotfiles-var";
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
            VarRecord::Create(env) => {
                encode::write_u8(&mut output, 0)?; // create

                env.serialize(&mut output)?;
            }
            VarRecord::Delete(env) => {
                encode::write_u8(&mut output, 1)?; // delete
                encode::write_array_len(&mut output, 1)?; // 1 field

                encode::write_str(&mut output, env.as_str())?;
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
            DOTFILES_VAR_VERSION => {
                let mut bytes = decode::Bytes::new(&data.0);

                let record_type = decode::read_u8(&mut bytes).map_err(error_report)?;

                match record_type {
                    // create
                    0 => {
                        let env = Var::deserialize(&mut bytes)?;
                        Ok(VarRecord::Create(env))
                    }

                    // delete
                    1 => {
                        let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;
                        ensure!(
                            nfields == 1,
                            "too many entries in v0 dotfiles var delete record"
                        );

                        let bytes = bytes.remaining_slice();

                        let (key, bytes) =
                            decode::read_str_from_slice(bytes).map_err(error_report)?;

                        if !bytes.is_empty() {
                            bail!("trailing bytes in encoded dotfiles var record. malformed")
                        }

                        Ok(VarRecord::Delete(key.to_owned()))
                    }

                    n => {
                        bail!("unknown Dotfiles var record type {n}")
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
pub struct VarStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl VarStore {
    // will want to init the actual kv store when that is done
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> VarStore {
        VarStore {
            store,
            host_id,
            encryption_key,
        }
    }

    pub async fn xonsh(&self) -> Result<String> {
        let env = self.vars().await?;

        let mut config = String::new();

        for env in env {
            config.push_str(&format!("${}={}\n", env.name, env.value));
        }

        Ok(config)
    }

    pub async fn fish(&self) -> Result<String> {
        let env = self.vars().await?;

        let mut config = String::new();

        for env in env {
            config.push_str(&format!("set -gx {} {}\n", env.name, env.value));
        }

        Ok(config)
    }

    pub async fn posix(&self) -> Result<String> {
        let env = self.vars().await?;

        let mut config = String::new();

        for env in env {
            if env.export {
                config.push_str(&format!("export {}={}\n", env.name, env.value));
            } else {
                config.push_str(&format!("{}={}\n", env.name, env.value));
            }
        }

        Ok(config)
    }

    pub async fn build(&self) -> Result<()> {
        let dir = atuin_common::utils::dotfiles_cache_dir();
        tokio::fs::create_dir_all(dir.clone()).await?;

        // Build for all supported shells
        let posix = self.posix().await?;
        let xonsh = self.xonsh().await?;
        let fsh = self.fish().await?;

        // All the same contents, maybe optimize in the future or perhaps there will be quirks
        // per-shell
        // I'd prefer separation atm
        let zsh = dir.join("vars.zsh");
        let bash = dir.join("vars.bash");
        let fish = dir.join("vars.fish");
        let xsh = dir.join("vars.xsh");

        tokio::fs::write(zsh, &posix).await?;
        tokio::fs::write(bash, &posix).await?;
        tokio::fs::write(fish, &fsh).await?;
        tokio::fs::write(xsh, &xonsh).await?;

        Ok(())
    }

    pub async fn set(&self, name: &str, value: &str, export: bool) -> Result<()> {
        if name.len() + value.len() > DOTFILES_VAR_LEN {
            return Err(eyre!(
                "var record too large: max len {} bytes",
                DOTFILES_VAR_LEN
            ));
        }

        let record = VarRecord::Create(Var {
            name: name.to_string(),
            value: value.to_string(),
            export,
        });

        let bytes = record.serialize()?;

        let idx = self
            .store
            .last(self.host_id, DOTFILES_VAR_TAG)
            .await?
            .map_or(0, |entry| entry.idx + 1);

        let record = atuin_common::record::Record::builder()
            .host(Host::new(self.host_id))
            .version(DOTFILES_VAR_VERSION.to_string())
            .tag(DOTFILES_VAR_TAG.to_string())
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
        if name.len() > DOTFILES_VAR_LEN {
            return Err(eyre!(
                "var record too large: max len {} bytes",
                DOTFILES_VAR_LEN,
            ));
        }

        let record = VarRecord::Delete(name.to_string());

        let bytes = record.serialize()?;

        let idx = self
            .store
            .last(self.host_id, DOTFILES_VAR_TAG)
            .await?
            .map_or(0, |entry| entry.idx + 1);

        let record = atuin_common::record::Record::builder()
            .host(Host::new(self.host_id))
            .version(DOTFILES_VAR_VERSION.to_string())
            .tag(DOTFILES_VAR_TAG.to_string())
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

    pub async fn vars(&self) -> Result<Vec<Var>> {
        let mut build = BTreeMap::new();

        // this is sorted, oldest to newest
        let tagged = self.store.all_tagged(DOTFILES_VAR_TAG).await?;

        for record in tagged {
            let version = record.version.clone();

            let decrypted = match version.as_str() {
                DOTFILES_VAR_VERSION => record.decrypt::<PASETO_V4>(&self.encryption_key)?,
                version => bail!("unknown version {version:?}"),
            };

            let ar = VarRecord::deserialize(&decrypted.data, version.as_str())?;

            match ar {
                VarRecord::Create(a) => {
                    build.insert(a.name.clone(), a);
                }
                VarRecord::Delete(d) => {
                    build.remove(&d);
                }
            }
        }

        Ok(build.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use rand::rngs::OsRng;

    use atuin_client::record::sqlite_store::SqliteStore;

    use crate::{shell::Var, store::test_local_timeout};

    use super::{VarRecord, VarStore, DOTFILES_VAR_VERSION};
    use crypto_secretbox::{KeyInit, XSalsa20Poly1305};

    #[test]
    fn encode_decode() {
        let record = Var {
            name: "BEEP".to_owned(),
            value: "boop".to_owned(),
            export: false,
        };
        let record = VarRecord::Create(record);

        let snapshot = [
            204, 0, 147, 164, 66, 69, 69, 80, 164, 98, 111, 111, 112, 194,
        ];

        let encoded = record.serialize().unwrap();
        let decoded = VarRecord::deserialize(&encoded, DOTFILES_VAR_VERSION).unwrap();

        assert_eq!(encoded.0, &snapshot);
        assert_eq!(decoded, record);
    }

    #[tokio::test]
    async fn build_vars() {
        let store = SqliteStore::new(":memory:", test_local_timeout())
            .await
            .unwrap();
        let key: [u8; 32] = XSalsa20Poly1305::generate_key(&mut OsRng).into();
        let host_id = atuin_common::record::HostId(atuin_common::utils::uuid_v7());

        let env = VarStore::new(store, host_id, key);

        env.set("BEEP", "boop", false).await.unwrap();
        env.set("HOMEBREW_NO_AUTO_UPDATE", "1", true).await.unwrap();

        let mut env_vars = env.vars().await.unwrap();

        env_vars.sort_by_key(|a| a.name.clone());

        assert_eq!(env_vars.len(), 2);

        assert_eq!(
            env_vars[0],
            Var {
                name: String::from("BEEP"),
                value: String::from("boop"),
                export: false,
            }
        );

        assert_eq!(
            env_vars[1],
            Var {
                name: String::from("HOMEBREW_NO_AUTO_UPDATE"),
                value: String::from("1"),
                export: true,
            }
        );
    }
}
