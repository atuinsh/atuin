# Desktop capture: storage

How the daemon stores what capture hands out: deltas within segments, the
generations of the store, deletion, and eviction.

## Storing snapshots

Every snapshot is the whole window, and capture must stay frequent to stay
responsive, so the saving has to come from storage. Three sequences were
captured and stored in several ways. The sizes are for the trees as JSON,
compressed with zstd at level 3:

| Sequence | Snapshots | Distinct | Each alone | Delta from the previous | One stream | One object per element |
|---|---|---|---|---|---|---|
| Firefox, a log page that grows every 1.5 s | 29 | 23 | 69.8 KB | 3.9 KB | 3.6 KB | 10.2 KB |
| Chromium, the same page | 25 | 25 | 90.3 KB | 4.6 KB | 4.5 KB | 11.7 KB |
| A settings dialog, six pages visited three times | 19 | 6 | 3.6 KB | 1.9 KB | 1.6 KB | 2.9 KB |

- **Identical snapshots are already free.** Going back to a page that did not
  change gives the same hash, so 13 of the 19 dialog snapshots cost one row
  each.
- **A delta from the previous snapshot is about 18 times smaller** than
  compressing each snapshot alone, for a page that changes a little at a time.
  A changed snapshot of the log page costs about 170 bytes.
- **One compressed stream is no better than deltas,** and a stream cannot drop
  its beginning.
- **One stored object per element is worse.** A parent with hundreds of
  children is stored again whenever one child changes.
- **The tree costs about 1.5 times the text** under every scheme.

The pieces divide the work this way:

- The library hands out whole trees with a hash. Compression needs earlier
  snapshots and a place to keep them, which belong to the daemon. Capture
  encodes each tree once, hashes those bytes, and the helper sends the bytes as
  they are. The daemon keeps the bytes for the delta, checks the hash, and
  decodes the tree only to render the text of a new content for the search
  index. It does both on a blocking thread, so that a large tree does not stall
  the daemon's runtime. A snapshot that found the same content as the snapshot
  before it carries only the hash. When the segment does not hold that content,
  because the write of the snapshot before failed, the daemon skips the
  snapshot and logs it once per segment.
- The daemon stores a snapshot as a delta from the previous distinct snapshot
  of the same segment. It stores a whole tree for the first distinct content of
  a segment and for every 32nd one. A segment can then be read and deleted
  without touching another. Dialogs, notifications, the active window's visits,
  and each background window in one run of the helper each have segments of
  their own.
- A background window's segment ends when the window's application, title, or
  URL changes, so that each segment says what its window showed. The segment
  also ends where its next content would be a whole tree, after an hour, and
  when the daemon has not written to it for an hour. The helper reads a
  background window again only when the window changes, so a stop of capture
  and the end of the helper's run end the segment at that moment. A segment
  that the daemon has not written to for an hour ends an hour after its last
  record. The disk budget evicts only segments that have ended, and a
  background segment would otherwise last for as long as the helper runs. The
  daemon keeps the last tree of each background segment as the base of the next
  delta, up to 32 MiB for all of them. A segment whose tree was dropped stores
  its next content whole.
- `encoding` gives a tree a compact binary form whose bytes are the same for
  the same tree. The hash of a snapshot is the hash of that form. The
  3,000-line page takes 215 KB in Firefox and 315 KB in Chromium, against 343
  KB and 790 KB as JSON. Compressed, it takes 8.1 KB and 9.7 KB, about what the
  rendered text takes. The deltas of the log page come to 3.6 KB for 25
  distinct snapshots in Firefox, and 4.1 KB in Chromium.
- Every record of the helper names its window by an id that the window keeps
  for the whole run, through title changes and when the user comes back to it.
  The daemon finds a background window's segment by that id. The store doesn't
  keep the id, because the id is valid only for one run of the helper. The id
  is a hint: a toolkit can hand the handle of a closed window to a new one.

## Keeping the store in order

The store keeps itself in order this way:

- **Record ids keep the order of writes.** A record's id follows the newest
  stored id when the clock reads earlier, so that key order stays the order of
  writes. The ids also follow the search index's reconciliation watermark,
  because the reconciler reads only the records after the watermark. Without
  that rule, a new id could fall below the watermark after the newest records
  were deleted and the clock was set back.
- **A read sees each segment as open or ended, as the store says.** A segment
  without an end record reads as open when the daemon holds the segment in its
  open segments. Otherwise an earlier daemon or helper run left the segment,
  and the segment reads as ended at its last record. A read takes its snapshot
  and copies the open segments in one hold of a lock. A write adds a segment to
  the open segments before it commits the segment's start, and removes the
  segment after it commits the segment's end. A segment that ends after a
  read's snapshot therefore reads as open, and a segment that starts during the
  read is left out or reads as open. The module docs of `store/open.rs` in
  `atuin-daemon` give the argument in full.
- **Upkeep and listings go on past damage.** Eviction, the sweep of open
  segments, the reconciler, and the listings of segments and recent activity
  log and skip a record that does not decode. An eviction removes a segment
  whose records are damaged whole, and removes a damaged record once no ended
  segment is left. A read of a snapshot whose content is damaged still fails.
- **A deletion by filter removes what damage hides.** A deletion of every
  application removes a segment whose start or end is damaged, and a damaged
  record that no segment lists, because the filter can select them. When the
  filter names an application, damage can hide whether the filter selects a
  segment or record. The deletion then keeps the segment or record, and the
  reply counts it, so that `atuin desktop-capture delete` warns about it. The
  preview of a deletion counts the damaged segments and records that the
  deletion removes, so that `delete --all` removes them even when no intact
  segment is left.
- **Evictions walk the segments, oldest first.** Each eviction starts where the
  last one kept a segment that is still open, and removes up to 64 segments per
  commit.
- **The store names its format.** Each database notes the format of its data in
  `meta`, and a new database notes the daemon's format. An open fails when the
  database holds a different format or a malformed one, because the daemon
  can't know what the records of another format mean. The daemon's status then
  shows capture as failed with that error.
- **Only the user can open the store.** The store's directory has mode 0700,
  because the store holds what capture read from every window.
- **A rewrite erases every trace of removed records.** fjall writes every value
  to its journal, and it starts a new journal only once the journal passes
  64 MB. A removed value therefore stays readable in the journal, and fjall 3.1
  doesn't have a way to make the journal turn over sooner. A rewrite copies the
  store's entries straight into the tables of a new database in
  `db-<n+1>.partial`, syncs the new database's files, and renames the directory
  to `db-<n+1>`. The rename commits the rewrite, and the rewrite then removes
  `db-<n>`. When that removal fails, the rewrite reports the failure, and the
  daemon tries the removal again every 15 minutes until it succeeds. An open
  uses the newest `db-<n>` and removes the other generations and every partial
  one, so a crash at any step leaves one complete store. A new store starts in
  `db-1`. The store holds a lock on `store.lock` while it is open, so that a
  second process can't remove a generation that the first one uses. Reads and
  writes wait when a rewrite is running. A rewrite fails before it copies
  anything when the disk has less space free than the store takes.
- **A rewritten database that fails to open keeps the store locked.** The store
  keeps `store.lock` and tries to open the new database twice more, within
  about a second. When the database still fails to open, every operation of the
  store fails with that error, and the daemon's status shows capture as failed.
  The operations try to open the database again at most once a minute.
- **A deletion rewrites the store before it answers.** The deletion runs
  between two runs of the helper, so the helper starts again only after the
  rewrite. Each transaction of a deletion also notes in `meta` that the store
  owes a rewrite, and the rewrite's copy leaves that note out. When the daemon
  stops before the rewrite, or when the rewrite fails, the store still owes the
  rewrite. The next open carries the rewrite out, and the daemon tries it again
  every 15 minutes.
- **The index follows each removal, even after a crash.** Each transaction that
  removes a segment from the store also notes in `meta` that the index owes the
  segment's removal, and the rewrite's copy keeps that note. A removal from the
  index after a deletion notes in the index's `scrub_owed` table that the index
  owes a scrub, and a scrub that succeeds clears that note. When the store's
  note is set, each open and the daemon's retry every 15 minutes remove from
  the index every segment that the store doesn't hold, and then clear the note.
  When the index's note is set, they scrub the index. Deletions and evictions
  hold the index's `follow_store` guard from the store's removal to the
  index's, so that one of them doesn't clear a note that the other one still
  needs.
- **Evictions rewrite the store now and then.** The store's estimate of its
  size starts at its size on disk when it opens, and fjall keeps the space of a
  removal on disk until a rewrite. Evictions therefore rewrite the store once
  the space that they freed since the last rewrite adds up to a quarter of the
  store, at most every two hours. Each transaction of an eviction adds the
  bytes that it frees to a count in `meta`, so the count outlives a restart.
  The rewrite's copy leaves the count out. A rewrite after an eviction also
  takes the evicted records out of fjall's journal, and the index's scrub after
  it takes their text out of the index. A failed rewrite after evictions
  doesn't fail the eviction, and the evictions wait the same two hours before
  they call for the next rewrite. The daemon also checks the count every 15
  minutes, so that a count from before a restart leads to a rewrite without a
  new eviction.

## Deleting

`atuin desktop-capture delete` removes whole segments. A deletion selects
either the segments that a filter of application and time range matches, or one
segment by the id of the segment or of any of its records. For example, the id
of a snapshot from `search` selects the snapshot's segment. The store's
`Selection` type has one variant for each, so that no layer has to reject an id
that comes with a range. A segment that overlaps the range only in part goes
whole, and so does the segment of a snapshot's id, because the secret that the
user wants gone can sit in any of the segment's snapshots.

A deletion pauses capture. The supervisor ends the helper's run, which closes
the open segments as the end of every run does, carries out the deletion, and
starts a new run. The usual case is a secret in the window that the user looks
at, and that window's segment is still open. Deleting an open segment while the
run goes on would need the store and the run to agree on the tree that the run
keeps as the base of the next delta. The supervisor also stops the reconciler
during the deletion, so that a pass that started before the deletion can't
index the deleted segments again.

The MCP tools only read, so agents can't delete. A deletion acts on the user's
data, and it stays a command that the user runs.
