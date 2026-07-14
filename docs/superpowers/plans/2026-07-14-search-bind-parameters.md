# Client Search Builder → sqlx Bind Parameters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the client-side SQLite search query builder (`Sqlite::search`) from inlining escaped values into the SQL string to real sqlx bind parameters (`?` placeholders + `.bind()`), so query correctness no longer depends on humans remembering the `quote()`-for-`_eq` vs raw-for-`_like` escaping contract.

**Architecture:** Keep `sql_builder` for query *structure* (it preserves WHERE insertion order and passes un-bound `?` through `.sql()` verbatim), but replace every inlined value with a `?` placeholder while collecting the values, in call order, into a `Vec<SqlValue>`. Because `sql_builder` emits WHERE conditions in insertion order, the placeholder order in the generated SQL matches the arg push order. The inner filtered query is still embedded twice in the dedup wrapper (deliberately — the code comments justify this structure for early-termination performance; we do **not** switch to a CTE, which SQLite may materialize and thereby change the query plan). For the dedup case we simply bind the args vector twice, in order.

**Tech Stack:** Rust, sqlx (SQLite), `sql_builder` v3.1.1, tokio test.

## Global Constraints

- Rust edition/toolchain: `rust-version = "1.97.0"` (workspace). Do not use features newer than this.
- **Behavior must not change.** All existing tests in `crates/atuin-client/src/database.rs` `mod test` (`test_search_prefix`, `test_search_fulltext`, `test_search_fuzzy`, `test_search_reordered_fuzzy`, `test_paged_*`, `test_search_bench_dupes`) must stay green after every task.
- Scope is limited to `crates/atuin-client/src/database.rs`. Do **not** touch the `list`, `all_with_count`, `stats`, or `Paged` query builders — they keep using `quote()` and remain valid, so `quote` and `SqlName` stay imported.
- Do **not** replace the double-embedded dedup subquery with a CTE. Preserve the exact `SELECT * FROM (inner) f WHERE NOT EXISTS (SELECT 1 FROM (inner) f2 ...)` shape.
- `LIMIT`/`OFFSET` values are `i64` and were never an injection risk; they stay inlined in `assemble_search_query`. The migration is about string values.
- Arg push order MUST equal the order WHERE conditions are added to the `SqlBuilder`. When adding a condition, push its value to `args` in the same statement region, before moving on to the next condition.

---

## File Structure

- Modify: `crates/atuin-client/src/database.rs`
  - Add module-level `enum SqlValue { Int(i64), Text(String) }`.
  - Add module-level `fn assemble_search_query(inner: &str, filter_options: &OptFilters) -> String` (Task 1).
  - Add module-level `fn build_inner_query(search_mode, filter, context, query, filter_options) -> (String, Vec<SqlValue>)` (Task 2).
  - Rewrite `Sqlite::search` to delegate to the two helpers and bind args (Tasks 1–2).
  - Change `SqlBuilderExt::fuzzy_condition` to emit a `?` and push to `args` (Task 2).
  - Change `apply_author_filter` to emit `?` for literal authors and push to `args` (Task 2).
  - Remove now-unused `esc` and `bind::Bind` imports (Task 2).
  - Add tests in the existing `#[cfg(test)] mod test` (`use super::*` already present).

---

## Task 1: Extract the outer-query assembly (`assemble_search_query`)

Behavior-preserving refactor: pull the dedup/order/limit wrapper out of `Sqlite::search` into a pure, unit-testable function. No bind parameters yet — the inner query is still built by the existing inlining code and passed in as a string.

**Files:**
- Modify: `crates/atuin-client/src/database.rs` — `Sqlite::search` tail (currently lines ~646–692); add new module-level fn near the other free functions (e.g. just after `get_session_start_time`, ~line 148).
- Test: `crates/atuin-client/src/database.rs` `mod test` (~line 1106+).

**Interfaces:**
- Produces: `fn assemble_search_query(inner: &str, filter_options: &OptFilters) -> String` — wraps `inner` in the ordering/limit/dedup outer query. Embeds `inner` **twice** when `filter_options.include_duplicates == false`, **once** when `true`.

- [ ] **Step 1: Write the failing test**

Add to `mod test`:

```rust
    #[test]
    fn assemble_search_query_embeds_inner_for_dedup_and_dup() {
        // dedup (default): inner appears twice
        let opts = OptFilters::default();
        let sql = assemble_search_query("SELECT 1", &opts);
        assert_eq!(sql.matches("SELECT 1").count(), 2, "dedup must embed inner twice");
        assert!(sql.contains("NOT EXISTS"));
        assert!(sql.contains("ORDER BY f.timestamp DESC"));

        // include_duplicates: inner appears once, no NOT EXISTS
        let opts_dup = OptFilters { include_duplicates: true, ..Default::default() };
        let sql_dup = assemble_search_query("SELECT 1", &opts_dup);
        assert_eq!(sql_dup.matches("SELECT 1").count(), 1);
        assert!(!sql_dup.contains("NOT EXISTS"));

        // reverse flips the order direction
        let opts_rev = OptFilters { reverse: true, ..Default::default() };
        assert!(assemble_search_query("SELECT 1", &opts_rev).contains("ORDER BY f.timestamp ASC"));

        // limit/offset are inlined as integers
        let opts_lim = OptFilters { limit: Some(5), offset: Some(2), ..Default::default() };
        assert!(assemble_search_query("SELECT 1", &opts_lim).contains("LIMIT 5 OFFSET 2"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p atuin-client assemble_search_query_embeds_inner_for_dedup_and_dup`
Expected: FAIL to compile — `cannot find function assemble_search_query in this scope`.

- [ ] **Step 3: Add the `assemble_search_query` function**

Add this module-level function (place it just after `get_session_start_time`):

```rust
fn assemble_search_query(inner: &str, filter_options: &OptFilters) -> String {
    let order = if filter_options.reverse { "ASC" } else { "DESC" };

    let tail = match (filter_options.limit, filter_options.offset) {
        (Some(limit), Some(offset)) => format!(" LIMIT {limit} OFFSET {offset}"),
        (Some(limit), None) => format!(" LIMIT {limit}"),
        // SQLite requires a LIMIT before OFFSET; -1 means "no limit".
        (None, Some(offset)) => format!(" LIMIT -1 OFFSET {offset}"),
        (None, None) => String::new(),
    };

    // Deduplicate by keeping, for each command, only its most recent entry
    // within the filtered set. Expressed as a correlated NOT EXISTS rather
    // than GROUP BY so that the timestamp-ordered scan can stop as soon as
    // `limit` distinct commands have been emitted. The inner query is embedded
    // twice on purpose; do not collapse it into a CTE (SQLite may materialize
    // it and lose the early-termination scan).
    if filter_options.include_duplicates {
        format!("SELECT * FROM ({inner}) f ORDER BY f.timestamp {order}{tail}")
    } else {
        format!(
            "SELECT * FROM ({inner}) f \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM ({inner}) f2 \
                 WHERE f2.command = f.command \
                   AND (f2.timestamp, f2.id) > (f.timestamp, f.id) \
             ) \
             ORDER BY f.timestamp {order}{tail}"
        )
    }
}
```

- [ ] **Step 4: Rewire `Sqlite::search` to call it**

Replace the tail of `search` (from `let order = ...` through the end of the `let query = if ... { ... } else { ... };` block) so the ordering/limit/dedup logic is gone and only this remains between the `inner` binding and the `sqlx::query` call:

```rust
        // sql_builder inlines every bound value, so the inner query carries no
        // positional parameters and is safe to embed (twice) as a derived table.
        let inner = sql.sql().expect("bug in search query. please report");
        let inner = inner.trim().trim_end_matches(';');

        let query = assemble_search_query(inner, &filter_options);

        let res = sqlx::query(&query)
            .map(Self::query_history)
            .fetch_all(&self.pool)
            .await?;

        Ok(ordering::reorder_fuzzy(search_mode, orig_query, res))
```

- [ ] **Step 5: Run the new test and the full search suite**

Run: `cargo test -p atuin-client assemble_search_query_embeds_inner_for_dedup_and_dup`
Expected: PASS

Run: `cargo test -p atuin-client test_search test_paged`
Expected: PASS (all existing search/paged tests still green)

- [ ] **Step 6: Commit**

```bash
git add crates/atuin-client/src/database.rs
git commit -m "refactor(client): extract assemble_search_query from search"
```

---

## Task 2: Migrate the inner query to bind parameters

Move the inner-query construction into a pure `build_inner_query` that emits `?` placeholders and returns the values in bind order, update `fuzzy_condition` and `apply_author_filter` to match, and bind the args (twice for dedup) in `search`. Remove the now-unused `esc`/`Bind` imports.

**Files:**
- Modify: `crates/atuin-client/src/database.rs` — import line (~14), `apply_author_filter` (~108–138), `Sqlite::search` body (~504–645 → replaced), `SqlBuilderExt`/`fuzzy_condition` (~975–1013), add `SqlValue` and `build_inner_query`.
- Test: `crates/atuin-client/src/database.rs` `mod test`.

**Interfaces:**
- Consumes: `assemble_search_query(&str, &OptFilters) -> String` (Task 1).
- Produces:
  - `enum SqlValue { Int(i64), Text(String) }`
  - `fn build_inner_query(search_mode: SearchMode, filter: FilterMode, context: &Context, query: &str, filter_options: &OptFilters) -> (String, Vec<SqlValue>)` — SQL string with `?` placeholders and the values in bind order. Invariant: `returned_sql.matches('?').count() == returned_args.len()`.
  - `SqlBuilderExt::fuzzy_condition(&mut self, field, mask: String, inverse, glob, is_or, args: &mut Vec<SqlValue>)` — pushes `mask` to `args`, emits `... [NOT] LIKE ?` / `... [NOT] GLOB ?`.
  - `apply_author_filter(sql: &mut SqlBuilder, authors: &[String], args: &mut Vec<SqlValue>)`.

- [ ] **Step 1: Write the failing unit test (placeholder/arg invariant + no inlining)**

Add to `mod test`:

```rust
    fn test_context() -> Context {
        Context {
            hostname: "test:host".to_string(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
            host_id: "test-host".to_string(),
            git_root: None,
        }
    }

    #[test]
    fn build_inner_query_binds_user_input() {
        let ctx = test_context();
        // A term containing a single quote used to require esc(); with binds it
        // must travel as a bound value, never inlined into the SQL text.
        let (sql, args) =
            build_inner_query(SearchMode::FullText, FilterMode::Global, &ctx, "foo'bar", &OptFilters::default());

        assert_eq!(sql.matches('?').count(), args.len(), "one bound value per placeholder");
        assert!(!sql.contains("foo'bar"), "user term must not be inlined into SQL");
        assert!(
            args.iter().any(|v| matches!(v, SqlValue::Text(s) if s.contains("foo'bar"))),
            "user term must be carried as a bound value"
        );
    }

    #[test]
    fn build_inner_query_placeholder_count_matches_args_with_filters() {
        let ctx = test_context();
        let opts = OptFilters {
            exit: Some(0),
            cwd: Some("/tmp".to_string()),
            authors: vec!["ellie".to_string()],
            ..Default::default()
        };
        let (sql, args) =
            build_inner_query(SearchMode::FullText, FilterMode::Session, &ctx, "cargo build", &opts);
        assert_eq!(sql.matches('?').count(), args.len());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p atuin-client build_inner_query`
Expected: FAIL to compile — `cannot find function build_inner_query` / `cannot find type SqlValue`.

- [ ] **Step 3: Add the `SqlValue` enum**

Add at module level (e.g. just after the `OptFilters` struct, ~line 62):

```rust
/// A value to be bound into a search query via sqlx, instead of being inlined
/// into the SQL text. Collected in placeholder order by `build_inner_query`.
#[derive(Debug, Clone)]
enum SqlValue {
    Int(i64),
    Text(String),
}
```

- [ ] **Step 4: Migrate `apply_author_filter` to bind literal authors**

Replace the whole `apply_author_filter` function with:

```rust
/// Each entry is OR'd: `$all-user` → NOT IN agents, `$all-agent` → IN agents, literal → exact match.
fn apply_author_filter(sql: &mut SqlBuilder, authors: &[String], args: &mut Vec<SqlValue>) {
    let mut conditions: Vec<String> = Vec::new();
    // KNOWN_AGENTS is a compile-time constant, so inlining it is safe.
    let agent_list: String = KNOWN_AGENTS.iter().map(quote).join(", ");
    let author_expr = "CASE \
        WHEN author IS NULL OR trim(author) = '' THEN \
            CASE \
                WHEN instr(hostname, ':') > 0 THEN substr(hostname, instr(hostname, ':') + 1) \
                ELSE hostname \
            END \
        ELSE author \
    END";

    for author in authors {
        match author.as_str() {
            AUTHOR_FILTER_ALL_USER => {
                conditions.push(format!("{author_expr} NOT IN ({agent_list})"));
            }
            AUTHOR_FILTER_ALL_AGENT => {
                conditions.push(format!("{author_expr} IN ({agent_list})"));
            }
            literal => {
                args.push(SqlValue::Text(literal.to_string()));
                conditions.push(format!("{author_expr} = ?"));
            }
        }
    }

    if !conditions.is_empty() {
        sql.and_where(format!("({})", conditions.join(" OR ")));
    }
}
```

- [ ] **Step 5: Migrate `fuzzy_condition` to emit a placeholder**

Replace the `SqlBuilderExt` trait and its impl with:

```rust
trait SqlBuilderExt {
    fn fuzzy_condition<S: ToString>(
        &mut self,
        field: S,
        mask: String,
        inverse: bool,
        glob: bool,
        is_or: bool,
        args: &mut Vec<SqlValue>,
    ) -> &mut Self;
}

impl SqlBuilderExt for SqlBuilder {
    /// adapted from the sql-builder *like functions, but binds `mask` instead of
    /// inlining it — the glob/like metacharacters in `mask` are intentional and
    /// pass through unescaped, exactly as before.
    fn fuzzy_condition<S: ToString>(
        &mut self,
        field: S,
        mask: String,
        inverse: bool,
        glob: bool,
        is_or: bool,
        args: &mut Vec<SqlValue>,
    ) -> &mut Self {
        let mut cond = field.to_string();
        if inverse {
            cond.push_str(" NOT");
        }
        if glob {
            cond.push_str(" GLOB ?");
        } else {
            cond.push_str(" LIKE ?");
        }
        args.push(SqlValue::Text(mask));
        if is_or {
            self.or_where(cond)
        } else {
            self.and_where(cond)
        }
    }
}
```

- [ ] **Step 6: Add `build_inner_query` and rewrite `search`**

Add this module-level function (place it just before `impl Database for Sqlite` or next to the other free functions):

```rust
fn build_inner_query(
    search_mode: SearchMode,
    filter: FilterMode,
    context: &Context,
    query: &str,
    filter_options: &OptFilters,
) -> (String, Vec<SqlValue>) {
    let mut sql = SqlBuilder::select_from("history");
    let mut args: Vec<SqlValue> = Vec::new();

    let git_root = if let Some(git_root) = context.git_root.clone() {
        git_root.to_str().unwrap_or("/").to_string()
    } else {
        context.cwd.clone()
    };

    let session_start = get_session_start_time(&context.session);

    match filter {
        FilterMode::Global => &mut sql,
        FilterMode::Host => {
            args.push(SqlValue::Text(context.hostname.to_lowercase()));
            sql.and_where_eq("lower(hostname)", "?")
        }
        FilterMode::Session => {
            args.push(SqlValue::Text(context.session.clone()));
            sql.and_where_eq("session", "?")
        }
        FilterMode::SessionPreload => {
            args.push(SqlValue::Text(context.session.clone()));
            sql.and_where_eq("session", "?");
            if let Some(session_start) = session_start {
                args.push(SqlValue::Int(session_start));
                sql.or_where_lt("timestamp", "?");
            }
            &mut sql
        }
        FilterMode::Directory => {
            args.push(SqlValue::Text(context.cwd.clone()));
            sql.and_where_eq("cwd", "?")
        }
        FilterMode::Workspace => {
            args.push(SqlValue::Text(format!("{git_root}%")));
            sql.and_where("cwd LIKE ?")
        }
    };

    let mut regexes = Vec::new();
    match search_mode {
        SearchMode::Prefix => {
            args.push(SqlValue::Text(format!("{}%", query.replace('*', "%"))));
            sql.and_where("command LIKE ?")
        }
        _ => {
            let mut is_or = false;
            for token in QueryTokenizer::new(query) {
                // TODO smart case mode could be made configurable like in fzf
                let (is_glob, glob) = if token.has_uppercase() {
                    (true, "*")
                } else {
                    (false, "%")
                };
                let param = match token {
                    QueryToken::Regex(r) => {
                        regexes.push(String::from(r));
                        continue;
                    }
                    QueryToken::Or => {
                        if !is_or {
                            is_or = true;
                            continue;
                        } else {
                            format!("{glob}|{glob}")
                        }
                    }
                    QueryToken::MatchStart(term, _) => format!("{term}{glob}"),
                    QueryToken::MatchEnd(term, _) => format!("{glob}{term}"),
                    QueryToken::MatchFull(term, _) => format!("{glob}{term}{glob}"),
                    QueryToken::Match(term, _) => {
                        if search_mode == SearchMode::FullText {
                            format!("{glob}{term}{glob}")
                        } else {
                            term.split("").join(glob)
                        }
                    }
                };

                sql.fuzzy_condition(
                    "command",
                    param,
                    token.is_inverse(),
                    is_glob,
                    is_or,
                    &mut args,
                );
                is_or = false;
            }

            &mut sql
        }
    };

    for regex in regexes {
        args.push(SqlValue::Text(regex));
        sql.and_where("command regexp ?");
    }

    if let Some(exit) = filter_options.exit {
        args.push(SqlValue::Int(exit));
        sql.and_where("exit = ?");
    }

    if let Some(exclude_exit) = filter_options.exclude_exit {
        args.push(SqlValue::Int(exclude_exit));
        sql.and_where("exit != ?");
    }

    if filter_options.only_failed {
        sql.and_where("exit != 0 AND exit != -1");
    }

    if let Some(cwd) = &filter_options.cwd {
        args.push(SqlValue::Text(cwd.clone()));
        sql.and_where("cwd = ?");
    }

    if let Some(exclude_cwd) = &filter_options.exclude_cwd {
        args.push(SqlValue::Text(exclude_cwd.clone()));
        sql.and_where("cwd != ?");
    }

    if let Some(before) = &filter_options.before {
        if let Ok(before) = interim::parse_date_string(
            before.as_str(),
            OffsetDateTime::now_utc(),
            interim::Dialect::Uk,
        ) {
            args.push(SqlValue::Int(before.unix_timestamp_nanos() as i64));
            sql.and_where("timestamp < ?");
        }
    }

    if let Some(after) = &filter_options.after {
        if let Ok(after) = interim::parse_date_string(
            after.as_str(),
            OffsetDateTime::now_utc(),
            interim::Dialect::Uk,
        ) {
            args.push(SqlValue::Int(after.unix_timestamp_nanos() as i64));
            sql.and_where("timestamp > ?");
        }
    }

    if !filter_options.authors.is_empty() {
        apply_author_filter(&mut sql, &filter_options.authors, &mut args);
    }

    sql.and_where_is_null("deleted_at");

    let inner = sql.sql().expect("bug in search query. please report");
    let inner = inner.trim().trim_end_matches(';').to_string();

    (inner, args)
}
```

Now replace the **entire body** of `Sqlite::search` (everything between the signature's opening `{` and closing `}`) with:

```rust
        let orig_query = query;

        let (inner, args) =
            build_inner_query(search_mode, filter, context, query, &filter_options);
        let sql = assemble_search_query(&inner, &filter_options);

        // The dedup wrapper embeds `inner` twice, so its placeholders appear
        // twice; bind the args vector once per copy, in order. The
        // include_duplicates wrapper embeds it once.
        let copies = if filter_options.include_duplicates { 1 } else { 2 };

        let mut q = sqlx::query(&sql);
        for _ in 0..copies {
            for value in &args {
                q = match value {
                    SqlValue::Int(i) => q.bind(*i),
                    SqlValue::Text(s) => q.bind(s.clone()),
                };
            }
        }

        let res = q
            .map(Self::query_history)
            .fetch_all(&self.pool)
            .await?;

        Ok(ordering::reorder_fuzzy(search_mode, orig_query, res))
```

- [ ] **Step 7: Remove unused imports**

Change line 14 from:

```rust
use sql_builder::{SqlBuilder, SqlName, bind::Bind, esc, quote};
```

to:

```rust
use sql_builder::{SqlBuilder, SqlName, quote};
```

- [ ] **Step 8: Run the new unit tests**

Run: `cargo test -p atuin-client build_inner_query`
Expected: PASS (both `build_inner_query_binds_user_input` and `build_inner_query_placeholder_count_matches_args_with_filters`).

- [ ] **Step 9: Run the full existing search/paged suite (regression gate)**

Run: `cargo test -p atuin-client test_search test_paged`
Expected: PASS — every pre-existing test still green, proving behavior is unchanged.

- [ ] **Step 10: Add an end-to-end escaping regression test**

Add to `mod test`:

```rust
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_handles_quotes() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        new_history_item(&mut db, "echo 'hello world'").await.unwrap();

        // A stored command containing a single quote is still matched.
        assert_search_commands(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "hello",
            vec!["echo 'hello world'"],
        )
        .await;

        // A query that itself contains a quote must not break the SQL — it just
        // matches nothing (previously this relied on esc(); now on binding).
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "no'such", 0)
            .await
            .unwrap();
    }
```

- [ ] **Step 11: Run the new e2e test**

Run: `cargo test -p atuin-client test_search_handles_quotes`
Expected: PASS

- [ ] **Step 12: Commit**

```bash
git add crates/atuin-client/src/database.rs
git commit -m "refactor(client): use sqlx bind parameters in search builder"
```

---

## Task 3: Full verification and lint cleanup

**Files:** none (verification only).

- [ ] **Step 1: Run the complete client test suite**

Run: `cargo test -p atuin-client`
Expected: PASS (no regressions anywhere in the crate).

- [ ] **Step 2: Lint — confirm no unused imports / warnings from the change**

Run: `cargo clippy -p atuin-client --all-targets -- -D warnings`
Expected: no errors. In particular, no "unused import: `esc`" / "`Bind`" — they were removed in Task 2 Step 7.

- [ ] **Step 3: Real smoke test against the built binary**

Run:
```bash
cargo run -p atuin -- search --limit 5 cargo
cargo run -p atuin -- search --limit 5 "echo | ls"
cargo run -p atuin -- search --limit 5 "r/^cd "
```
Expected: each returns results (or an empty result set) without a SQL error, matching the behavior on `main`. If you have history containing a `'`, also try `cargo run -p atuin -- search "'"` and confirm it does not error.

- [ ] **Step 4: Commit (only if Step 2 required any formatting/lint fixup)**

```bash
git add -A
git commit -m "chore(client): lint cleanup after search bind migration"
```

---

## Self-Review

**1. Spec coverage** — the request was "migrate the client search builder to sqlx bind parameters."
- Filter-mode clauses (Host/Session/SessionPreload/Directory/Workspace): migrated in `build_inner_query` (Task 2 Step 6). ✔
- Command matching (Prefix / fuzzy tokens / regex): Prefix + regex in `build_inner_query`; fuzzy in `fuzzy_condition` (Task 2 Steps 5–6). ✔
- Scalar filters (exit, exclude_exit, only_failed, cwd, exclude_cwd, before, after): Task 2 Step 6. ✔
- Author filter: `apply_author_filter` (Task 2 Step 4). ✔
- Outer dedup/order/limit wrapper: `assemble_search_query`, structure preserved (Task 1). ✔
- Binding once vs twice for the double-embedded inner: `copies` loop in `search` (Task 2 Step 6). ✔
- Unused-import cleanup: Task 2 Step 7. ✔

**2. Placeholder scan** — no "TBD"/"handle edge cases"/"similar to"; every code step shows complete code.

**3. Type consistency** —
- `assemble_search_query(&str, &OptFilters) -> String`: defined Task 1, called Task 2 Step 6. ✔
- `build_inner_query(SearchMode, FilterMode, &Context, &str, &OptFilters) -> (String, Vec<SqlValue>)`: defined and called in Task 2, matches the test calls in Step 1. ✔
- `SqlValue { Int(i64), Text(String) }`: defined Task 2 Step 3; constructed in `build_inner_query`/`fuzzy_condition`/`apply_author_filter`; matched in the bind loop and tests. ✔
- `fuzzy_condition(..., mask: String, ..., args: &mut Vec<SqlValue>)`: definition (Step 5) and call site (Step 6) agree; `param` passed is a `String`. ✔
- `apply_author_filter(&mut SqlBuilder, &[String], &mut Vec<SqlValue>)`: definition (Step 4) and call (Step 6) agree. ✔

**Key invariant guarded by tests:** `sql.matches('?').count() == args.len()`, and arg push order == WHERE insertion order == placeholder textual order (relied upon because `sql_builder` stores `wheres: Vec<String>` in insertion order and `or_where` appends to the tail element).

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-14-search-bind-parameters.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
