hub-linked = Successfully linked CLI account to Hub
theme-color-invalid = tried to load string as a color unsuccessfully: ({ $name }={ $color }) { $error }
theme-name-mismatch = your theme config name is not the name of your loaded theme { $name } != { $config_name }
theme-load-failed = could not load theme { $name }: { $error }
logout-done = You have logged out!
logout-not-logged-in = You are not logged in
plugin-update-rollback-failed = failed to roll back the update, you may need to reinstall Atuin: { $error }
sync-version-mismatch = version mismatch: to sync, the server needs to run a newer version of Atuin (client: { $client }, server: { $server })
settings-path-empty = { $key } is empty, so Atuin uses its default
settings-path-unexpanded = { $key }: failed to expand { $raw } ({ $error }), so Atuin uses it as written; write $$ for a literal $
record-store-decrypt-failed = Failed to decrypt { $id }, deleting
sync-upload-start = Uploading { $count } records to { $host }/{ $tag }
sync-download-start = Downloading { $count } records from { $host }/{ $tag }
sync-packfile-unexpandable = skipping unexpandable packfile manifest { $manifest } (host { $host }, idx { $idx }): { $error }. you have lost data.
history-records-skipped = skipped { $skipped } history records that could not be decrypted or decoded; run `atuin store verify` to check your store, and `atuin store purge` to remove broken records locally
import-bash-time-reversal = time reversal detected in Bash history; commands may be ordered incorrectly
