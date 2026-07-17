# End-to-end sync benchmark: baseline results

Measured on: Apple M5 Pro, arm64, macOS 26.5.2, rustc 1.97.0 (2d8144b78 2026-07-07)
Command: `cargo bench -p atuin-client --bench benchmarks -- sync 2>/tmp/atuin-bench.stderr`
Corpus: 10,000 records, ~600 bytes each on the wire.

## Results

| direction | page size | injected RTT | round-trips | median   |
| --- | --- | --- | --- | --- |
| upload | 100 | 0ms | 100 | 213.4 ms |
| upload | 1000 | 0ms | 10 | 196.1 ms |
| upload | 100 | 20ms | 100 | 2.628 s |
| upload | 1000 | 20ms | 10 | 407.1 ms |
| upload | 100 | 100ms | 100 | 10.8 s |
| upload | 1000 | 100ms | 10 | 1.345 s |
| download | 100 | 0ms | 100 | 151.6 ms |
| download | 1000 | 0ms | 10 | 141.5 ms |
| download | 100 | 20ms | 100 | 2.595 s |
| download | 1000 | 20ms | 10 | 362.3 ms |
| download | 100 | 100ms | 100 | 10.46 s |
| download | 1000 | 100ms | 10 | 1.216 s |

## Reading these numbers

- **The `rtt=0ms` rows are the whole argument for this design.** page=100 vs page=1000 differ by
  only ~8% on upload (213.4ms vs 196.1ms) and ~7% on download (151.6ms vs 141.5ms) at rtt=0. A
  pure-localhost benchmark would have reported PR #3584's page-size change as noise — it is only
  once real network latency is injected that the win appears.
- The `rtt=20ms` and `rtt=100ms` rows are where the change pays. The gap between page=100 and
  page=1000 approaches `90 × rtt` — the 90 round-trips removed by going from 100 pages to 10:
  - upload, rtt=100ms: gap is 10.8s − 1.345s = 9.455s, against a 9s prediction (90 × 100ms).
  - download, rtt=100ms: gap is 10.46s − 1.216s = 9.244s, against the same 9s prediction.
  - At rtt=20ms the gaps (2.221s upload, 2.233s download) run somewhat above the 1.8s prediction
    (90 × 20ms), which is expected — the prediction only accounts for injected latency, not the
    fixed per-request client/server overhead that page=100 pays 90 more times than page=1000 does.
- Latency is injected server-side per request. This models RTT only: bandwidth stays
  loopback-fast, so these numbers are a *lower bound* on the real-world win. A real WAN link also
  pays serialization delay on a 600 KB page, which favours smaller pages slightly.
- **Download is faster than upload at rtt=0**, which is the opposite of what the plan predicted
  (it expected download to be slower "since each page is also written into the client's SQLite
  store"). Both directions actually write all 10,000 rows into a WAL-mode SQLite database — upload
  into the server's, download into the client's — so there was no structural basis for that
  prediction. The likely explanation is that the server's `add_records` write path (a `uuid_v7()`
  call per row, plus an `insert ... on conflict do nothing` with 10 bind parameters) is heavier
  than the client's `push_batch` path. Read as an observation, not a proven cause: at rtt=0, the
  server's write path — not the client's — appears to dominate.
- **Don't over-read precision on the page=1000 numbers.** page=1000 completes in only 10
  round-trips at `sample_count = 5`, so its per-round-trip overhead is noisy: dividing the rtt=20ms
  and rtt=100ms gaps above by 90 gives ~2.5ms/rt and ~14.9ms/rt respectively, versus page=100's
  much more stable ~4ms/rt implied overhead. Treat the page=1000 rows as accurate to ratios and
  orders of magnitude, not to four significant figures.

## Caveats

- **Always redirect stderr.** `sync_upload`/`sync_download` draw an `indicatif` progress bar that
  suppresses itself only when stderr is not a TTY. On a terminal it adds work proportional to page
  count, which distorts exactly the comparison being made. Sync's progress reporting is not
  injectable, which is what forces this workaround.
- The server runs SQLite, not the Postgres that production uses. This keeps server-side variance
  out of the measurement, but it means these numbers do not predict server-side load.
- Payloads are random bytes, not real PASETO ciphertext. Nothing on the sync path decrypts, so
  this is invisible to the code under test — but it does mean record size is a fixed ~600-byte
  assumption rather than a real distribution.

## Follow-ups this benchmark surfaced

- **axum's 2 MB default body limit bounds page size.** `handlers::v0::record::post` extracts
  `Json<Vec<Record<EncryptedData>>>` and `atuin-server` never overrides `DefaultBodyLimit`. At
  this benchmark's ~600 bytes/record, page=1000 is ~600 KB and safe. A user with 1–2 KB records
  would send 1–2 MB and could get a `413`. Before raising the page size, either bound the request
  by bytes rather than record count, or raise the server's limit deliberately.
- Sync's progress reporting is not injectable, which is what forces the stderr caveat above.
  Threading a quiet/draw-target option through `sync_remote` would make this benchmark
  reproducible by construction.
