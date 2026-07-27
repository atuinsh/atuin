# End-to-end sync benchmark: baseline results

Measured on: Apple M5 Pro, arm64, macOS 26.5.2, rustc 1.97.0 (2d8144b78 2026-07-07)
Note: this run post-dates the fix to `benches/sync.rs` where `bench_values` was dropping the
sample's `TempDir`/`SqliteStore` guards inside the timed region (divan's `drop_input` is a
no-op for `bench_values`; the guards are now returned as outputs so divan drops them after
`sample_end`). A run from before that fix will show different rtt=0 medians.
Command: `cargo bench -p atuin-client --bench benchmarks -- sync 2>/tmp/atuin-bench.stderr`
Corpus: 10,000 records, ~600 bytes each on the wire.

## Results

| direction | page size | injected RTT | round-trips | median   |
| --- | --- | --- | --- | --- |
| upload | 100 | 0ms | 100 | 219.5 ms |
| upload | 1000 | 0ms | 10 | 199 ms |
| upload | 100 | 20ms | 100 | 2.606 s |
| upload | 1000 | 20ms | 10 | 431.1 ms |
| upload | 100 | 100ms | 100 | 10.71 s |
| upload | 1000 | 100ms | 10 | 1.31 s |
| download | 100 | 0ms | 100 | 147.8 ms |
| download | 1000 | 0ms | 10 | 132.5 ms |
| download | 100 | 20ms | 100 | 2.506 s |
| download | 1000 | 20ms | 10 | 380 ms |
| download | 100 | 100ms | 100 | 10.55 s |
| download | 1000 | 100ms | 10 | 1.253 s |

## Reading these numbers

- **The `rtt=0ms` rows are the whole argument for this design.** page=100 vs page=1000 differ by
  only ~9% on upload (219.5ms vs 199ms) and ~10% on download (147.8ms vs 132.5ms) at rtt=0. A
  pure-localhost benchmark would have reported PR #3584's page-size change as noise — it is only
  once real network latency is injected that the win appears.
- The `rtt=20ms` and `rtt=100ms` rows are where the change pays. The gap between page=100 and
  page=1000 approaches `90 × rtt` — the 90 round-trips removed by going from 100 pages to 10:
  - upload, rtt=100ms: gap is 10.71s − 1.31s = 9.40s, against a 9s prediction (90 × 100ms).
  - download, rtt=100ms: gap is 10.55s − 1.253s = 9.297s, against the same 9s prediction.
  - At rtt=20ms the gaps (2.175s upload, 2.126s download) run somewhat above the 1.8s prediction
    (90 × 20ms), which is expected — the prediction only accounts for injected latency, not the
    fixed per-request client/server overhead that page=100 pays 90 more times than page=1000 does.
- Latency is injected server-side per request. This models RTT only: bandwidth stays
  loopback-fast, so these numbers are a *lower bound* on the real-world win. A real WAN link also
  pays serialization delay on a 600 KB page, which favours smaller pages slightly.
- **Download is faster than upload at rtt=0**, which is the opposite of what the plan predicted
  (it expected download to be slower "since each page is also written into the client's SQLite
  store"). Both directions actually write all 10,000 rows into a WAL-mode SQLite database — upload
  into the server's, download into the client's — so there was no structural basis for that
  prediction. One candidate explanation is that the server's `add_records` write path (a
  `uuid_v7()` call per row, plus an `insert ... on conflict do nothing` with 10 bind parameters)
  is heavier than the client's `push_batch` path — though the client's `save_raw` also does three
  `as_hyphenated().to_string()` allocations per row that the server's direct `Uuid` binds avoid,
  which cuts against leaning on the bind-parameter count as the reason. At least as plausible a
  confound: the upload bench's server-side database is reused and churned across samples (all
  10,000 rows deleted then re-inserted each sample, against a WAL that grows sample over sample),
  while the download bench's client-side database is a brand-new, empty temp file every sample —
  so upload may be measuring a warmer, larger, more fragmented database rather than (or in
  addition to) a heavier write path. Read as an observation, not a proven cause: at rtt=0,
  something on the server side of the round trip — write path, database reuse, or both — appears
  to dominate.
- **Don't over-read precision on the page=1000 numbers.** Decomposing each row as
  `(median − round_trips × rtt) / round_trips` gives the implied per-round-trip overhead.
  For page=1000 (10 round-trips): upload rtt=20 → (431.1−200)/10 = 23.1ms/rt; upload rtt=100 →
  (1310−1000)/10 = 31.0ms/rt; download rtt=20 → (380−200)/10 = 18.0ms/rt; download rtt=100 →
  (1253−1000)/10 = 25.3ms/rt — a noisy ~18.0–31.0ms/rt range. For page=100 (100 round-trips):
  upload rtt=20 → (2606−2000)/100 = 6.06ms/rt; upload rtt=100 → (10710−10000)/100 = 7.10ms/rt;
  download rtt=20 → (2506−2000)/100 = 5.06ms/rt; download rtt=100 → (10550−10000)/100 = 5.50ms/rt
  — a much more stable ~5.06–7.10ms/rt. The main reason isn't just sampling noise, it's
  structural: the fixed per-sample work visible directly in the rtt=0 rows — roughly 132–220ms,
  ranging from 132.5ms (download, page=1000) up to 219.5ms (upload, page=100) — gets amortized
  over only 10 round-trips at page=1000 versus 100 at page=100, which inflates and destabilizes
  the per-round-trip figure — and each page=1000 request also carries roughly 10x the bytes. The
  per-round-trip decomposition is therefore not a meaningful unit for page=1000. Treat the
  page=1000 rows as accurate to ratios and orders of magnitude, not to four significant figures.

## Caveats

- **Always redirect stderr.** `sync_upload`/`sync_download` draw an `indicatif` progress bar that
  suppresses itself only when stderr is not a TTY. On a terminal it adds work proportional to page
  count, which distorts exactly the comparison being made. Sync's progress reporting is not
  injectable, which is what forces this workaround.
- The server runs SQLite, not the Postgres that production uses. This keeps server-side variance
  out of the measurement, but it means these numbers do not predict server-side load.
- Payloads are random bytes, not real PASETO ciphertext. Nothing on the sync path decrypts, so
  this is invisible to the code under test — but it does mean the encrypted-data payload is a
  fixed 300-byte assumption (`PAYLOAD_SIZE` in `crates/atuin-client/benches/_util/record.rs:25`,
  plus a 150-byte `KEY_SIZE` wrapped key at line 28) rather than a real distribution. That's the
  fixed assumption a reader checking the source will find; it's smaller than the ~600-byte total
  on-the-wire record size quoted elsewhere in this document, which also includes the UUIDs and
  JSON framing around the payload.

## Follow-ups this benchmark surfaced

- **axum's 2 MB default body limit bounds page size.** `handlers::v0::record::post` extracts
  `Json<Vec<Record<EncryptedData>>>` and `atuin-server` never overrides `DefaultBodyLimit`. At
  this benchmark's ~600 bytes/record, page=1000 is ~600 KB and safe. A user with 1–2 KB records
  would send 1–2 MB and could get a `413`. Before raising the page size, either bound the request
  by bytes rather than record count, or raise the server's limit deliberately.
- Sync's progress reporting is not injectable, which is what forces the stderr caveat above.
  Threading a quiet/draw-target option through `sync_remote` would make this benchmark
  reproducible by construction.
