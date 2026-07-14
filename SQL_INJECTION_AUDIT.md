# SQL Injection Audit — atuin

Date: 2026-07-14 (rewritten after the `refactor/less-scary-sql` bind-parameter
migration).
Scope: full workspace (`crates/*`), focus on every path where a SQL string is
built dynamically rather than passed as a constant to sqlx.

## Verdict

**No exploitable SQL injection found, but the workspace is not uniformly
parameterized.** The original client search builder (`Sqlite::search` and its
helpers in `crates/atuin-client/src/database.rs`) was never actually
injectable — every interpolated value was escaped via `sql_builder`'s
`esc`/`quote`, or bound with real sqlx `.bind()`. That code has since been
migrated (this branch) so the search path is now fully parameterized with `?`
placeholders instead of relying on escaping. `list`/`stats`/`Paged`/
`all_with_count` deliberately still use `quote()`-based inlining, and remain
correct.

Separately, this rewrite found one raw-`format!` SQL site in
`crates/atuin/src/command/client/search/engines/daemon.rs` that interpolates a
value with **no escaping and no quoting at all**. It is not currently
exploitable, but only because of an invariant enforced by its one caller, not
because of anything in the vulnerable function itself — see §5 below. This is
the one item in this audit that should actually be fixed, not just recorded.

## The escaping contract (why the scary-looking `quote()`-based code is safe)

`sql_builder` v3.1.1 (`crates/atuin-client` depends on it) provides:

- `esc(s)` → `s.replace("'", "''")`. Single-quote doubling is the *complete and
  only* escaping needed inside a SQLite/standard-SQL single-quoted string
  literal (there is no backslash escape to worry about), so this is correct.
- `quote(s)` → `'{esc(s)}'` (escape + wrap in quotes).
- `.bind(&x)` on a `str`/`String` → `quote(x)`; on an integer → the bare number.
- `and_where_like_left(field, mask)` / `and_where_like` → build
  `field LIKE '{esc(mask)}%'`, i.e. they **escape the mask internally**, so the
  caller passes the *raw* value.
- `and_where_eq(field, value)` / `_ne` / `_lt` / `_gt` → emit `field = {value}`
  with **no escaping**, so the caller **must** pre-quote string values.

`list`, `stats`, `Paged::next`, and `all_with_count` in
`crates/atuin-client/src/database.rs` still use this contract, consistently:
`_eq`/`_ne`/`_lt`/`_gt` always get `quote(...)` for strings (or a bare `i64`);
`_like_left` gets a raw string. This split from `search` (see §1) is
deliberate and is now called out in a code comment above `use sql_builder` and
above `list` in that file — do not copy a `"?"`-placeholder pattern from the
search builder into these functions without also adding a matching `.bind()`.

## Records

### 1. Client search query builder — `crates/atuin-client/src/database.rs` (current state)

This is the fully-parameterized path added by this branch. No `esc`/`quote`
calls remain anywhere in it; `sql_builder`'s `esc` is not even imported (only
`SqlBuilder`, `SqlName`, and `quote` are, and `quote` is used solely by the
still-inlined functions in §3).

- `build_inner_query` (L388–551) assembles the inner `SELECT` with
  `SqlBuilder`, using `"?"` placeholders for every user-controlled value
  (`and_where_eq(field, "?")`, `and_where("command LIKE ?")`, etc.) and
  collecting the corresponding values into an ordered `Vec<SqlValue>` (`args`),
  an enum of `Int(i64)`/`Text(String)` (L67–71). No user value is ever
  interpolated into the SQL text.
- `apply_author_filter` (L123–148) follows the same pattern: literal values
  (`$all-user`/`$all-agent`) expand to fixed clauses referencing the
  compile-time `KNOWN_AGENTS` constant; a free-text author value is pushed to
  `args` and bound via `?`.
- `SqlBuilderExt::fuzzy_condition` (L1058–1096) — the fuzzy/glob search-term
  path — used to be "the load-bearing escape" (`esc(mask)` before a closing
  quote). It's now bind-only: it builds `field LIKE ?` / `field NOT GLOB ?`
  and pushes `mask` to `args` instead of ever touching the SQL string. The
  glob/like metacharacters in `mask` (`%`, `_`, `*`) are intentionally left
  unescaped and pass straight through to SQLite's `LIKE`/`GLOB`, exactly as
  before the migration — that's a feature (it's what makes `*` a wildcard in
  fuzzy search), not a gap.
- `assemble_search_query` (L173–216) wraps `build_inner_query`'s output for
  ordering/limit/dedup. It embeds the caller-supplied `inner` SQL string
  either once (`include_duplicates`) or twice (the `NOT EXISTS` dedup branch)
  and returns `(String, usize)` — the assembled SQL plus exactly how many
  times `inner` (and therefore its placeholders) was embedded. `order` is a
  fixed `"ASC"`/`"DESC"` literal; `limit`/`offset` are `i64`, inlined directly
  (not bound) since they are never attacker-controlled strings.
- `Sqlite::search` (L740–772) runs the assembled SQL via `sqlx::query(&sql)`
  and binds `args` once per `embeds` (the count returned by
  `assemble_search_query`, not recomputed independently — see the
  `debug_assert_eq!` on the bind arity right before executing). Every
  placeholder in the final SQL has a corresponding bound value.

### 2. `list` / `stats` / `Paged::next` / `all_with_count` — same file

These four were deliberately **not** migrated to bind parameters and still use
`SqlBuilder` + `quote()`-based inlining, run as raw strings via
`sqlx::query(&query)`:

- `list` (L622–675): `quote(&context.hostname)`, `quote(&context.session)`,
  `quote(&context.cwd)` for `_eq` filters; `and_where_like_left("cwd", &git_root)`
  passes the raw string (the helper escapes internally).
- `stats` (L856+): seven `SqlBuilder` queries containing `strftime(...)` and
  `?1`/`?2` placeholders — these ones already use real sqlx `.bind()` calls,
  not `quote()`. The `strftime` format strings are hardcoded constants, not
  user input.
- `Paged::next` (L1025–1057): `quote(last_id)` for the cursor `_lt` filter.
- `all_with_count` (L786+): no user-controlled interpolation at all — all
  fields/group-by/order clauses are fixed literals.

Correct as-is. Kept as raw-string `SqlBuilder` output rather than migrated,
because these are lower-traffic/simpler call sites where the `quote()`
contract is easy to keep straight — see the caution comment left in the code
about not mixing the two styles.

### 3. `query_history(query: &str)` / `sqlite_version` — same file

`Database::query_history` (trait method, L254; `Sqlite` impl, L777) executes
an arbitrary SQL string. A code smell (a public method that runs raw SQL), but
every caller *within `atuin-client`* feeds it `SqlBuilder`-generated,
already-escaped/parameter-free SQL (`Paged::next` at L1057). See §5 below,
though, for callers **outside** `atuin-client` that do not uphold this
invariant.

### 4. Import readers — `crates/atuin-client/src/import/`

`xonsh_sqlite.rs` and `zsh_histdb.rs` call `sqlx::query(query)` where `query`
is a **constant** `&str` (e.g. `"SELECT COUNT(*) FROM xonsh_history"`). The
`sqlx::query(db_sql)` in `zsh_histdb.rs` is hardcoded fixture SQL inside a
`#[test]`. No interpolation.

### 5. Unescaped raw-`format!` callers of `query_history` outside `atuin-client` (NEW — needs a fix)

`Database::query_history` is a public trait method; nothing stops a caller
elsewhere in the workspace from building unsafe SQL and passing it in. Three
such callers exist, with differing risk:

**a. `crates/atuin/src/command/client/search/engines/daemon.rs`,
`hydrate_from_db` (L104–111) — unescaped, unquoted interpolation, currently
safe only by an unstated caller invariant.**

```rust
async fn hydrate_from_db(&self, db: &dyn Database, ids: &[String]) -> Result<Vec<History>> {
    let placeholders: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
    let sql_query = format!(
        "SELECT * FROM history WHERE id IN ({}) ORDER BY timestamp DESC",
        placeholders.join(",")
    );
    Ok(db.query_history(&sql_query).await?)
}
```

Each `id` is wrapped in single quotes by `format!("'{id}'")` with **zero
escaping** — if `id` ever contained a `'`, it would break out of the string
literal into the surrounding SQL. This is currently not exploitable only
because the sole caller (`full_query`, same file, ~L188–190) always builds
`ids` by re-serializing daemon-supplied UUID bytes:

```rust
Uuid::from_bytes(bytes).as_simple().to_string()
```

`as_simple()` always yields exactly 32 lowercase hex characters — a value
space with no `'` in it — so today's only call path is safe. But
`hydrate_from_db` takes `ids: &[String]`, not `&[Uuid]`, and `HistoryId` itself
(`crates/atuin-client/src/history.rs:48`, `pub struct HistoryId(pub String)`)
has no validation on construction. The daemon builds a `HistoryId` directly
from a gRPC request field with no format check
(`crates/atuin-daemon/src/components/history.rs`, `end_history`, ~L189:
`let id = HistoryId(req.id);`). Nothing in the type system or in
`hydrate_from_db` itself prevents a future caller from passing an arbitrary
string through this path — the safety property lives entirely in "the one
current caller happens to only ever construct 32-hex strings," which is easy
to silently break in a refactor.

**Recommendation: bind the ids** instead of inlining them, e.g. build
`"SELECT * FROM history WHERE id IN (?,?,...)"` with one placeholder per id
and bind each `id` as a string, or (simpler, given `query_history` takes a
plain string) add a dedicated `Database` method that accepts `&[String]` and
binds them internally. This removes the reliance on the caller-side
hex-only invariant entirely.

**b. `crates/atuin/src/command/client/search/interactive.rs`,
`DeleteAllMatching` (~L1865–1880) — hand-rolled escaping, safe but fragile.**

```rust
let all_matching = db.query_history(
    &format!(
        "select * from history where command = '{}' and deleted_at is null",
        command.replace('\'', "''")
    )
).await?;
```

`command.replace('\'', "''")` is the same single-quote-doubling rule
`sql_builder::esc` implements, so this is currently safe. It's a hand-rolled,
uncommented duplicate of that escaping rule rather than a call to `quote()` or
a bound parameter, though — a future edit could easily "clean up" the
`.replace()` call without realizing it's load-bearing. Low priority (no bug
today), but bind or `quote()`-wrap this rather than leaving the manual
`.replace()` as the only defense.

**c. `crates/atuin-daemon/src/components/search.rs` (~L192–205) — not
injectable, but suspected pre-existing latent bug (never matches anything).**

```rust
format!(
    "select * from history where id in ({})",
    records
        .iter()
        .map(|record| record.0.to_string())
        .collect::<Vec<_>>()
        .join(",")
)
```

`records: &[RecordId]` and `RecordId` is a `Uuid` newtype
(`crates/atuin-common/src/lib.rs`, `new_uuid!` macro), so `record.0.to_string()`
cannot contain a `'` — not injectable regardless of the missing quotes.
However: the ids are interpolated **completely unquoted** (`id in
(a1b2c3d4-...,...)`, not `id in ('a1b2c3d4-...',...)`), which is very likely a
SQL syntax error today (unquoted hyphenated tokens aren't valid SQL). Even if
it somehow parsed, `record.0.to_string()` on a `Uuid` produces the
**hyphenated** form (`8-4-4-4-12`), while `history.id` is stored as the
**32-char `as_simple()` hex** form used everywhere else in this codebase (see
§1's `hydrate_from_db` above, and `HistoryId` construction sites) — so even
with correct quoting this would never match a real `history.id`. It also
conflates *record* ids (the sync/record-store namespace) with *history* ids
(a separate table's primary key) as if they were interchangeable, which they
are not. This looks like a pre-existing functional bug (this event handler
likely silently does nothing / always queries zero rows), not a security
issue — flagging separately from the injection audit for someone to look at.

## Confirmed-safe (parameterized, not even looks-injectable)

Independently re-verified; unchanged from the prior audit.

- **Server — `crates/atuin-server-postgres/src/lib.rs`**: every query is a
  constant string with `$N` placeholders + `.bind()`. The only `format!` calls
  build error-message strings, not SQL. This is the multi-tenant,
  network-facing surface — and it is clean.
- **Server — `crates/atuin-server-sqlite/src/lib.rs`**: same pattern (`?N` +
  `.bind()`).
- **`crates/atuin-kv/src/database.rs`**, **`crates/atuin-scripts/src/database.rs`**:
  all queries are constant strings with `?N` placeholders and `.bind()`.
- **`crates/atuin-server-database/src/calendar.rs`**: no SQL; pure date types.

## Recommendations (in priority order)

1. **Fix `hydrate_from_db`** (§5a) — bind the ids instead of `format!("'{id}'")`
   interpolation. This is the one place where safety depends entirely on an
   invariant (`HistoryId` values always being 32-hex) that isn't enforced by
   the type, so it should not be left as "safe today, fragile tomorrow."
2. Replace the hand-rolled `command.replace('\'', "''")` in
   `DeleteAllMatching` (§5b) with a bound parameter or `sql_builder::quote()`,
   so the escaping isn't a silent, uncommented `.replace()` call.
3. Look into the suspected latent bug in `crates/atuin-daemon/src/components/search.rs`
   (§5c) — unquoted, wrong-format ids suggest this `RecordsAdded` handler may
   never actually inject any rows into the search index. Separate from this
   audit's scope, but worth a follow-up ticket.
4. Document the "callers must pass builder output only, or a
   provably-safe/bound string" invariant on `Database::query_history`, or
   narrow its visibility/replace raw-string callers with typed methods, given
   §5 shows it already has at least one caller violating that invariant in
   spirit.
