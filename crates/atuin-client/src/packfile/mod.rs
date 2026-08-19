//! Packfile creation logic within atuin.
//!
//! # Context
//!
//! Packfiles are Atuin's internal representation of multiple equivalently-tagged,
//! equivalently-hosted history records.
//!
//! Atuin normally stores history entries in a global records table with the structure:
//!
//! ```txt
//! | idx | tag     | data (encrypted) |
//! |   1 | history | ...              |
//! |   2 | history | ...              |
//! |   3 | history | ...              |
//! |   4 | history | ...              |
//! ```
//!
//! This, however, introduces a set of problems -- since each record is encrypted and stored in
//! Postgres, two downsides happen:
//!
//!   - There is a large amount of overhead storing each of the records separately. Normally, you
//!     could compress multiple history entries quite aggressively, encrypting after they are
//!     compressed.
//!   - Each record has to get downloaded individually, which means that a huge amount of network
//!     traffic is incurred only to transfer data.
//!
//! We therefore desire to implement shared bundling and packing, bundling multiple history records
//! together.
//!
//! # Design
//!
//! The solution is to create a _packfile_ -- a collection of shared records. Naively, you'd expect
//! to be able to combine this data together in the server and call it a day, but that doesn't
//! work. As it turns out, compressing already encrypted data is ineffective.
//!
//! Rather, we need to compress data first, and then encrypt it after the fact. Consequently, it
//! must be the client (as only the client has access to the key), which performs the compression.
//!
//! ## Packing
//!
//! The first piece of the puzzle is packing. Every time a new entry is added to the local records
//! table, the local client invokes [`packer::try_pack`] which is responsible for finding the last
//! pack point, and then deducing whether further packing should be done.
//!
//! Note that the server advertises the packfile size through [`atuin_domain::caps::PackfileCap`].
//!
//! Consider the aforementioned example. Assuming that indices `[1, 4]` were already in the records
//! from before packfiles were merged, adding a new `idx = 5` causes [`packer::try_pack`] to be
//! invoked, which will identify that no previous pack exists. This will cause it to perform a
//! packing operation, which will create a new `PackManifestData` record in the table.
//!
//! ```txt
//! | idx | tag      | data (encrypted)         |
//! |   1 | history  | ...                      |
//! |   2 | history  | ...                      |
//! |   3 | history  | ...                      |
//! |   4 | history  | ...                      |
//! |   5 | history  | ...                      |
//! |   6 | packfile | { "start": 1, "end": 2 } |
//! |   7 | packfile | { "start": 3, "end": 4 } |
//! ```
//!
//! **No negotiation with the server has occurred up until this point.** We consider the packing
//! phase to be complete. Subsequent `history` additions will invoke [`packer::try_pack`] which will
//! hold off on adding entries until the server-advertised record count has been reached (in the
//! following example -- `2`):
//!
//! ```txt
//! | idx | tag      | data (encrypted)         |
//! |   1 | history  | ...                      |
//! |   2 | history  | ...                      |
//! |   3 | history  | ...                      |
//! |   4 | history  | ...                      |
//! |   5 | history  | ...                      |
//! |   6 | packfile | { "start": 1, "end": 2 } |
//! |   7 | packfile | { "start": 3, "end": 4 } |
//! |   8 | history  | ...                      |
//! |   9 | packfile | { "start": 5, "end": 8 } |
//! |  10 | history  | ...                      |
//! ```
//!
//! ## Sync
//!
//! It is only during syncing that a packfile is actually created as a binary object, and shuttled
//! to the remote.
//!
//! ### Uploading
//!
//! During the uploading loop in [`crate::record::sync`], when we find the packed record, we invoke
//! the [`sync::upload_packed`] procedure, which is responsible for performing the actual packing
//! and uploading payload.
//!
//! This operation will open up the manifest in the packfile, read the `"start"` and `"end"` values,
//! and scan the record table for these entries. For each of these entries, it will decrypt them,
//! bundle them, compress them and then encrypt them. With this now encrypted packfile, it will
//! upload it to the server.
//!
//! ### Downloading
//!
//! During the downloading loop in [`crate::record::sync`], when we find a remote `packfile`
//! manifest record, we invoke [`sync::download_packed`], which is responsible for turning that
//! manifest back into local `history` records.
//!
//! This operation asks the server for the packfile's presigned download URL, fetches the packfile,
//! unpacks it (via `PackManifestRecordView::unpack_records`) back into the individual history
//! records for the manifest's `"start".."end"` range, re-encrypts each one with the local key, and
//! pushes them into the local record store.
mod packer;
mod record;
mod sync;

pub use packer::{PackingError, try_pack};
#[cfg(test)]
pub(crate) use record::PackManifestRecordView;
pub use sync::{DownloadError, UploadError, download_packed, upload_packed};
