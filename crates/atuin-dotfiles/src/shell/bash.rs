use std::path::PathBuf;

use crate::store::AliasStore;

async fn cached_aliases(path: PathBuf, store: &AliasStore) -> String {
    match tokio::fs::read_to_string(path).await {
        Ok(aliases) => aliases,
        Err(r) => {
            // we failed to read the file for some reason, but the file does exist
            // fallback to generating new aliases on the fly

            store.posix().await.unwrap_or_else(|e| {
                format!("echo 'Atuin: failed to read and generate aliases: \n{r}\n{e}'",)
            })
        }
    }
}

/// Return bash dotfile config
///
/// Do not return an error. We should not prevent the shell from starting.
///
/// In the worst case, Atuin should not function but the shell should start correctly.
///
/// While currently this only returns aliases, it will be extended to also return other synced dotfiles
pub async fn config(store: &AliasStore) -> String {
    // First try to read the cached config
    let aliases = atuin_common::utils::dotfiles_cache_dir().join("aliases.bash");

    if aliases.exists() {
        return cached_aliases(aliases, store).await;
    }

    if let Err(e) = store.build().await {
        return format!("echo 'Atuin: failed to generate aliases: {}'", e);
    }

    cached_aliases(aliases, store).await
}
