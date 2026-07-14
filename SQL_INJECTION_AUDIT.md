# SQL Injection Audit — atuin

Date: 2026-07-14
Scope: full workspace (`crates/*`), focus on every path where a SQL string is
built dynamically rather than passed as a constant to sqlx.

## Verdict

**No SQL injection vulnerabilities found.** Every dynamically-built query routes
user-controlled string values through `sql_builder`'s escaping (`esc`/`quote`,
or the internal escaping of the `*_like_*` helpers), or uses real sqlx bind
parameters (`?N` / `$N` + `.bind()`). Several call sites *look* injectable
because they concatenate SQL with `format!`/`push_str`, but the interpolated
values are either escaped strings or Rust integers. They are recorded below so a
future reviewer doesn't re-flag them — and so the invariant that keeps them safe
is written down.

## The escaping contract (why the scary-looking code is safe)

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

atuin uses this contract consistently: `_eq`/`_ne`/`_lt`/`_gt` always get
`quote(...)` for strings (or a bare `i64`); `_like_left` gets a raw string.

## Records — looks-injectable-but-safe

### 1. Client search query builder — `crates/atuin-client/src/database.rs`
The largest surface. `Sqlite::search` (≈L504–692) assembles the query with
`SqlBuilder`, `format!`, and manual string pushes, then runs it via
`sqlx::query(&query)` with no bind params (L686). Safe because:

- `FilterMode` clauses (L526–541): string values wrapped in `quote(...)` for
  `_eq`; `and_where_like_left("cwd", git_root)` passes raw (helper escapes).
- Fuzzy/glob search terms (L546–594) flow into the custom `fuzzy_condition`
  (L986–1012), which builds `command LIKE '…'` / `command GLOB '…'` using
  `esc(mask)` before the closing quote (L1005–1006). User terms are escaped.
- Regex tokens (L596–598): `"command regexp ?".bind(&regex)` → `.bind` quotes
  and escapes the regex string.
- `exit` / `exclude_exit` are `Option<i64>`; `only_failed` is a fixed literal
  clause; `cwd`/`exclude_cwd` are `quote(...)`d; `before`/`after` are parsed to
  timestamps and cast to `i64`.
- Author filter (L108–138): `quote(literal)` for the user value; agent list is
  from the compile-time `KNOWN_AGENTS` constant.
- Outer dedup/order/limit wrapper (L657–684): `order` is a fixed `"ASC"`/`"DESC"`
  literal; `limit`/`offset` are `i64`; `inner` is the already-escaped subquery.

### 2. `SqlBuilderExt::fuzzy_condition` — same file, L986–1012
Hand-rolls `field LIKE '<mask>'` by string concatenation. Safe *only* because of
`esc(mask.to_string())` at L1005. **This is the load-bearing escape** — if a
future edit drops the `esc()` call, the search box becomes injectable. Worth a
comment/test guarding it.

### 3. `list` / `all_with_count` / `Paged::query` — same file (L~380–443, 700–738, 900–972)
Built with `SqlBuilder` and run as raw strings. String values use `quote(...)`
(e.g. `quote(&context.cwd)`, `quote(last_id)`); fields/group-by/order are fixed
literals.

### 4. `stats` — same file, L773–869
Builds seven `SqlBuilder` queries containing `strftime(...)` and `?1/?2`
placeholders, then binds with real sqlx `.bind()` (L835+). The `strftime` format
strings are hardcoded constants, not user input. Fully parameterized.

### 5. `query_history(query: &str)` / `sqlite_version` — same file
`query_history` (L694–699) and the `Database::query_history` trait method
execute an arbitrary SQL string. A code smell (a public method that runs raw
SQL), but every caller feeds it `SqlBuilder`-generated, already-escaped SQL. No
external caller passes attacker-controlled SQL. Candidate for a doc-comment
noting the "callers must pass builder output only" invariant.

### 6. Import readers — `crates/atuin-client/src/import/`
`xonsh_sqlite.rs` and `zsh_histdb.rs` call `sqlx::query(query)` where `query` is
a **constant** `&str` (e.g. `"SELECT COUNT(*) FROM xonsh_history"`). The
`sqlx::query(db_sql)` in `zsh_histdb.rs` L234 is hardcoded fixture SQL inside a
`#[test]`. No interpolation.

## Confirmed-safe (parameterized, not even looks-injectable)

- **Server — `crates/atuin-server-postgres/src/lib.rs`**: every query is a
  constant string with `$N` placeholders + `.bind()`. The only `format!` calls
  (L58, L87) build error-message strings, not SQL. This is the multi-tenant,
  network-facing surface — and it is clean.
- **Server — `crates/atuin-server-sqlite/src/lib.rs`**: same pattern (`?N` +
  `.bind()`).
- **`crates/atuin-kv/src/database.rs`**, **`crates/atuin-scripts/src/database.rs`**:
  all queries are constant strings with `?N` placeholders and `.bind()`.
- **`crates/atuin-server-database/src/calendar.rs`**: no SQL; pure date types.

## Recommendations (hardening, not fixes)

1. Add a comment + unit test around `fuzzy_condition` (L1005) asserting that a
   term containing `'` cannot terminate the literal — this is the single most
   fragile escape in the codebase.
2. Document the "builder output only" invariant on `Database::query_history`, or
   narrow its visibility, so it can't grow an attacker-reachable caller.
3. (Optional) Consider migrating the client search builder to sqlx bind
   parameters. The current approach is correct but relies on humans keeping the
   `quote()`-for-`_eq` vs raw-for-`_like` distinction straight on every edit.
