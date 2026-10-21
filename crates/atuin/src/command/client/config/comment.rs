//! Comment-based processing of Atuin's config file.

use std::ops::Range;

use atuin_common::string::StrExt;
use toml_edit::{Document, Item, RawString, Table};

use super::TomlPath;

/// Whitespace characters as defined by the TOML spec.
const TOML_WHITESPACE: [char; 2] = [' ', '\t'];

/// Enable a commented-out assignment of `path` so that the TOML document actually contains `path`.
///
/// If the document doesn't already contain `path`, this function searches the comments in the
/// document for a commented-out assignment to `path` in the appropriate table. For example, if
/// `path` is `daemon.sync_frequency`, this will look for a line like `# sync_frequency = ...` below
/// the `[daemon]` header (and before the next header).
///
/// If found, one of two strategies will be used:
///
/// * The example line will be uncommented by removing the `# ` prefix.
///
/// * A dummy assignment to the path will be inserted below the comment block in which the example
///   line was found. "Comment block" here means a contiguous block of comments with no blank lines
///   in between.
///
/// The uncommenting approach is used only when deemed safe, to avoid overwriting user data.
/// Uncommenting requires the following conditions to be true:
///
/// * There is exactly one space after the `#` in the example line.
/// * The example line is the last line in its comment block.
/// * Every preceding line in the comment block begins with `## `, and there is at least one such
///   line.
/// * The example line is a valid TOML assignment, and the value is not a table or array (`atuin
///   config set` can't write those types, and the risk of overwriting user data is higher).
///
/// This catches the majority of commented-out examples in the default `config.toml`. If the
/// conditions aren't met, the insertion approach is used, which puts the new assignment in a
/// relevant location without performing any potentially destructive operation.
///
/// Returns the updated TOML document, or [`None`] if no edit was made.
pub fn enable_example(config: &str, path: TomlPath<'_>) -> Option<String> {
    let doc = Document::parse(config).ok()?;
    if path.0.iter().try_fold(doc.as_item(), |item, name| item.as_table_like()?.get(name)).is_some()
    {
        // Document already contains `path`.
        return None;
    }

    let mut parent = path;
    let leaf = parent.0.pop()?;

    if !is_bare_key(leaf) {
        // Only look for bare keys. This makes the processing easier, and `atuin config set` can
        // only set bare keys anyway.
        return None;
    }

    let edit = comment_spans(&doc, &parent)?
        .filter_map(|span| get_edit(config, span, leaf))
        .min_by_key(Edit::index)?;

    Some(match edit {
        Edit::Uncomment(prefix) => {
            String::from_iter([&config[..prefix.start], &config[prefix.end..]])
        }
        Edit::Insert(index) => {
            // Insert a placeholder assignment to 0; the caller will overwrite this.
            let (before, after) = config.split_at(index);
            format!("{}\n{leaf} = 0\n{after}", before.strip_suffix('\n').unwrap_or(before))
        }
    })
}

/// Whether `c` can appear in an unquoted TOML key.
fn is_bare_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

/// Whether `key` can be written in TOML without quotes.
fn is_bare_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(is_bare_key_char)
}

/// How to edit the TOML document so that it contains an uncommented assignment of the key.
enum Edit {
    /// Remove the specified range in order to uncomment the example assignment of the key.
    ///
    /// The range corresponds to the example's leading `# `.
    Uncomment(Range<usize>),

    /// Insert a fresh assignment to the key at the specified index.
    Insert(usize),
}

impl Edit {
    /// The index at which the edit will be made.
    const fn index(&self) -> usize {
        match self {
            Self::Uncomment(range) => range.start,
            Self::Insert(index) => *index,
        }
    }
}

/// Every span of comments in `doc` associated with the given table.
///
/// "Associated with" means that if a bare `key = value` were inserted at the location of the
/// comment, that key-value assignment would be parsed as belonging to the table at `table_path`.
fn comment_spans(
    doc: &Document<&str>,
    table_path: &TomlPath<'_>,
) -> Option<impl Iterator<Item = Range<usize>>> {
    let mut tables = tables_in_order(doc).into_iter();
    let table = tables.find_map(|(path, table)| (path? == *table_path).then_some(table))?;

    // The next table's prefix decor (or the document's trailing decor, if this is the last table)
    // is conceptually part of this table.
    let tail = if let Some((_, next_table)) = tables.next() {
        next_table.decor().prefix().and_then(RawString::span)
    } else {
        doc.trailing().span()
    };

    Some(child_comment_spans(table).into_iter().chain(tail))
}

/// The spans of all the comments before keys in `table`.
fn child_comment_spans(table: &Table) -> Vec<Range<usize>> {
    fn visit(table: &Table, spans: &mut Vec<Range<usize>>) {
        for (name, item) in table {
            if let Item::Table(child) = item
                && child.is_dotted()
            {
                // We need to traverse dotted tables in order to capture comments above assignments
                // like `a.b.c = true`.
                visit(child, spans);
            } else if item.is_value() {
                let span = table
                    .key(name)
                    .and_then(|key| key.leaf_decor().prefix())
                    .and_then(RawString::span);
                spans.extend(span);
            }
        }
    }

    let mut spans = Vec::new();
    visit(table, &mut spans);
    spans
}

/// All the tables in `doc`, including the root, in the order they appear in the source.
///
/// Returns a vector of `(table_path, table)`. Tables that are inside arrays (`[[table]]`) cannot be
/// accessed by path, so their paths are [`None`] instead.
fn tables_in_order<'doc>(doc: &'doc Document<&str>) -> Vec<(Option<TomlPath<'doc>>, &'doc Table)> {
    fn visit<'doc>(
        table: &'doc Table,
        path: Option<&TomlPath<'doc>>,
        tables: &mut Vec<(Option<TomlPath<'doc>>, &'doc Table)>,
    ) {
        // Include the root table even though it doesn't have a position. `None` sorts before `Some`
        // so the order will still be correct.
        if table.position().is_some() || path == Some(&TomlPath::root()) {
            tables.push((path.cloned(), table));
        }

        for (name, item) in table {
            match item {
                Item::Table(child) => {
                    let child_path = path.map(|path| path.child(name));
                    visit(child, child_path.as_ref(), tables);
                }
                Item::ArrayOfTables(array) => {
                    for child in array {
                        visit(child, None, tables);
                    }
                }
                _ => {}
            }
        }
    }

    let mut tables = Vec::new();
    visit(doc.as_table(), Some(&TomlPath::root()), &mut tables);
    tables.sort_by_key(|(_, table)| table.position());
    tables
}

/// Search a range of comments for a place where an assignment to `leaf_key` can be inserted.
///
/// `span` is the range within `config` to search. It should consist only of comments and
/// whitespace. If this range is not a suitable location for an assignment to `leaf_key`, this
/// function returns [`None`].
fn get_edit(config: &str, span: Range<usize>, leaf_key: &str) -> Option<Edit> {
    let comments = &config[span.clone()];
    let mut lines = comments.line_indices();
    let (example_start, example) = lines
        .find_map(|(i, line)| {
            if is_commented_out_header(line) {
                // The rest of the comments belong to a commented-out table. We no longer write
                // these in the default config, but older configs might have them.
                Some(Err(()))
            } else if is_example_of(line, leaf_key) {
                Some(Ok((i, line)))
            } else {
                None
            }
        })?
        .ok()?;

    let example_end = lines.next().map_or(comments.len(), |(i, _)| i);
    let block_end = comments[example_end..]
        .line_indices()
        .find(|(_, line)| line.is_space_or_empty())
        .map_or(comments.len(), |(i, _)| example_end + i);

    // In order to uncomment the example line:
    // Example line must be the last line in the comment block.
    let edit = if block_end == example_end
        // Example line must start with "# " followed by the key.
        && let Some(body) = example.strip_prefix("# ")
        && body.starts_with(is_bare_key_char)
        // All preceding lines in the comment block must start with "## ", and there must be at
        // least one such line.
        && comments[..example_start]
            .lines()
            .rev()
            .take_while(|line| !line.is_space_or_empty())
            .try_fold(false, |_, line| line.starts_with("## ").then_some(true))
            == Some(true)
        // The example line must be a valid TOML assignment of a scalar value.
        && is_scalar_assignment(body)
    {
        let absolute_start = span.start + example_start;
        Edit::Uncomment(absolute_start..absolute_start + "# ".len())
    } else {
        Edit::Insert(span.start + block_end)
    };

    Some(edit)
}

/// Whether `line` is a commented-out assignment of `key`, like `# key = 123`.
fn is_example_of(line: &str, key: &str) -> bool {
    line.strip_prefix('#')
        .and_then(|body| body.trim_start_matches(TOML_WHITESPACE).strip_prefix(key))
        .is_some_and(|rest| rest.trim_start_matches(TOML_WHITESPACE).starts_with('='))
}

/// Whether `line` is a commented-out table header, like `# [daemon]`.
fn is_commented_out_header(line: &str) -> bool {
    line.trim_start().strip_prefix('#').is_some_and(|body| body.trim_start().starts_with('['))
}

/// Whether `line` is a whole `key = value` assignment that `atuin config set` can write.
fn is_scalar_assignment(line: &str) -> bool {
    Document::parse(line).is_ok_and(|example| {
        example.iter().all(|(_, item)| {
            // Exclude arrays and tables -- `atuin config set` can't write them, and we risk losing
            // too much information if we overwrite the example.
            item.as_value().is_some_and(|value| !value.is_array() && !value.is_inline_table())
        })
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use atuin_client::settings::Settings;
    use rstest::rstest;

    use super::*;

    fn enable(config: &str, key: &str) -> Option<String> {
        enable_example(config, TomlPath::parse_dotted_simple(key))
    }

    #[rstest]
    #[case::under_an_array_of_tables("[[dirs]]\n## d\n# path = \"/\"\n", "dirs.path")]
    #[case::after_an_array_of_tables(
        "[[dirs]]\npath = \"/\"\n## d\n# auto_sync = true\n",
        "auto_sync"
    )]
    #[case::with_a_dotted_key("## d\n# sync.records = true\n", "sync.records")]
    #[case::under_a_commented_out_table_header(
        "[ui]\n\n# [pty_proxy]\n## d\n# enabled = false\n",
        "pty_proxy.enabled"
    )]
    #[case::of_another_table_under_a_commented_out_header(
        "[daemon]\n# [pty_proxy]\n## d\n# enabled = false\n",
        "daemon.enabled"
    )]
    #[case::of_another_table_under_an_indented_commented_out_header(
        "[daemon]\n  #  [pty_proxy]\n## d\n# enabled = false\n",
        "daemon.enabled"
    )]
    #[case::under_a_quoted_table_name_with_a_dot("[\"a.b\"]\n## d\n# x = 1\n", "a.b.x")]
    #[case::after_a_quoted_table_name_with_a_dot("[\"a.b\"]\nx = 1\n## d\n# y = 2\n", "y")]
    #[case::among_dotted_keys("sync.a = 1\n## d\n# sync.b = 2\nsync.c = 3\n", "sync.b")]
    #[case::in_the_previous_tables_section(
        "x = 1\n## d\n# sync_frequency = 300\n[daemon]\n",
        "daemon.sync_frequency"
    )]
    #[case::in_the_next_tables_section("[daemon]\n## d\n# x = 1\n", "x")]
    #[case::above_its_table_under_a_commented_out_header(
        "# [theme]\n## d\n# name = \"autumn\"\n\n[theme]\ndebug = true\n",
        "theme.name"
    )]
    #[case::already_set("## d\n# auto_sync = true\nauto_sync = false\n", "auto_sync")]
    #[case::already_set_as_a_table("## d\n# sync = true\n[sync]\n", "sync")]
    #[case::indented("## d\n  # auto_sync = true\n", "auto_sync")]
    #[case::only_in_prose("## e.g. auto_sync = true\n", "auto_sync")]
    #[case::of_a_longer_key("## d\n# auto_sync_x = true\n", "auto_sync")]
    #[case::without_an_equals_sign("## d\n# auto_sync true\n", "auto_sync")]
    fn examples_left_alone(#[case] input: &str, #[case] key: &str) {
        assert_eq!(enable(input, key), None);
    }

    /// A key that would have to be quoted can't be matched as it's written in an example.
    #[rstest]
    fn examples_of_a_key_with_a_dot_left_alone() {
        let path = TomlPath::root().child("a.b");
        assert_eq!(enable_example("## d\n# a.b = 1\n", path), None);
    }

    #[rstest]
    #[case::at_the_root(
        "## sync\n# auto_sync = true\n",
        "auto_sync",
        "## sync\nauto_sync = true\n"
    )]
    #[case::below_several_descriptions_with_extra_whitespace(
        "## sync\n##   \tif true\n# auto_sync = true\n",
        "auto_sync",
        "## sync\n##   \tif true\nauto_sync = true\n"
    )]
    #[case::with_a_trailing_comment(
        "[sync]\n## d\n# frequency=\"5m\" # or 0\n",
        "sync.frequency",
        "[sync]\n## d\nfrequency=\"5m\" # or 0\n"
    )]
    #[case::before_a_key(
        "[sync]\n## d\n# frequency = \"5m\"\nrecords = true\n",
        "sync.frequency",
        "[sync]\n## d\nfrequency = \"5m\"\nrecords = true\n"
    )]
    #[case::before_a_blank_line(
        "[sync]\nrecords = true\n## d\n# frequency = \"5m\"\n\n## e\n[ui]\n",
        "sync.frequency",
        "[sync]\nrecords = true\n## d\nfrequency = \"5m\"\n\n## e\n[ui]\n"
    )]
    #[case::after_a_blank_line(
        "# notes\n\n## d\n# auto_sync = true\n",
        "auto_sync",
        "# notes\n\n## d\nauto_sync = true\n"
    )]
    #[case::among_dotted_keys(
        "sync.a = 1\n## d\n# frequency = \"5m\"\nsync.c = 3\n",
        "frequency",
        "sync.a = 1\n## d\nfrequency = \"5m\"\nsync.c = 3\n"
    )]
    #[case::among_nested_dotted_keys_under_a_table(
        "[t]\na.b.c = 1\n## d\n# x = 2\na.b.d = 3\n",
        "t.x",
        "[t]\na.b.c = 1\n## d\nx = 2\na.b.d = 3\n"
    )]
    #[case::first_of_examples_around_dotted_keys(
        "sync.a = 1\n## d\n# y = 1\nx = 1\n## d\n# y = 2\nsync.c = 3\n",
        "y",
        "sync.a = 1\n## d\ny = 1\nx = 1\n## d\n# y = 2\nsync.c = 3\n"
    )]
    #[case::without_a_trailing_newline(
        "[sync]\n## d\n# frequency = \"5m\"",
        "sync.frequency",
        "[sync]\n## d\nfrequency = \"5m\""
    )]
    fn uncomments(#[case] input: &str, #[case] key: &str, #[case] expected: &str) {
        assert_eq!(enable(input, key).as_deref(), Some(expected));
    }

    /// A quoted table name with a dot in it can't be named by a dotted key, but can by a path.
    #[rstest]
    fn uncomments_under_a_quoted_table_name_with_a_dot() {
        let path = TomlPath::root().child("a.b").child("x");
        assert_eq!(
            enable_example("[\"a.b\"]\n## d\n# x = 1\n", path).as_deref(),
            Some("[\"a.b\"]\n## d\nx = 1\n")
        );
    }

    #[rstest]
    #[case::without_a_description("# x = 1\n", "# x = 1\nx = 0\n")]
    #[case::below_a_single_hash_comment("# d\n# x = 1\n", "# d\n# x = 1\nx = 0\n")]
    #[case::below_an_empty_description("##\n# x = 1\n", "##\n# x = 1\nx = 0\n")]
    #[case::below_a_description_without_a_space("##d\n# x = 1\n", "##d\n# x = 1\nx = 0\n")]
    #[case::below_an_indented_description("  ## d\n# x = 1\n", "  ## d\n# x = 1\nx = 0\n")]
    #[case::below_another_example("## d\n# y = 1\n# x = 1\n", "## d\n# y = 1\n# x = 1\nx = 0\n")]
    #[case::followed_by_comments("## d\n# x = 1\n## more\n", "## d\n# x = 1\n## more\nx = 0\n")]
    #[case::followed_by_more_examples(
        "## Default:\n# x = 1\n##\n## Examples:\n# x = 2\n\ny = 1\n",
        "## Default:\n# x = 1\n##\n## Examples:\n# x = 2\nx = 0\n\ny = 1\n"
    )]
    #[case::up_to_a_blank_line(
        "# x = 1\n# more\n\n# other\ny = 1\n",
        "# x = 1\n# more\nx = 0\n\n# other\ny = 1\n"
    )]
    #[case::before_an_indented_key("# x = 1\n  y = 2\n", "# x = 1\nx = 0\n  y = 2\n")]
    #[case::without_a_trailing_newline("## d\n# x = 1\n# more", "## d\n# x = 1\n# more\nx = 0\n")]
    #[case::without_a_space("## d\n#x=1\n", "## d\n#x=1\nx = 0\n")]
    #[case::with_two_spaces("## d\n#  x = 1\n", "## d\n#  x = 1\nx = 0\n")]
    #[case::with_a_tab("## d\n#\tx = 1\n", "## d\n#\tx = 1\nx = 0\n")]
    #[case::of_an_array("## d\n# x = [1]\n", "## d\n# x = [1]\nx = 0\n")]
    #[case::of_an_inline_table("## d\n# x = { a = 1 }\n", "## d\n# x = { a = 1 }\nx = 0\n")]
    #[case::of_a_multi_line_array(
        "## d\n# x = [\n#   1,\n# ]\n\ny = 1\n",
        "## d\n# x = [\n#   1,\n# ]\nx = 0\n\ny = 1\n"
    )]
    #[case::that_isnt_valid_toml(
        "## d\n# x = 1 is too slow\n",
        "## d\n# x = 1 is too slow\nx = 0\n"
    )]
    #[case::before_a_later_described_example(
        "# x = 1\n\n## d\n# x = 2\n",
        "# x = 1\nx = 0\n\n## d\n# x = 2\n"
    )]
    fn inserts_after_the_comment_block(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(enable(input, "x").as_deref(), Some(expected));
    }

    /// Every example in the default config sets its key in the right table, by uncommenting it or
    /// inserting the key after it within the table's section, with nothing else changing. Most are
    /// uncommented.
    #[rstest]
    fn enables_each_example_in_the_default_config() {
        let config = Settings::example_config();
        let lines: Vec<&str> = config.lines().collect();
        let mut table = TomlPath::root();
        let mut seen = HashSet::new();
        let mut uncommented = 0;

        for (i, line) in lines.iter().enumerate() {
            if let Some(header) = line.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
                table = TomlPath::parse_dotted_simple(header);
                continue;
            }
            assert!(
                !is_commented_out_header(line),
                "line {i} comments out a table header; write it out so `config set` can set the \
                 table's keys"
            );

            let Some(key) = line
                .strip_prefix('#')
                .and_then(|body| body.split_once('='))
                .map(|(key, _)| key.trim())
                .filter(|key| is_bare_key(key))
            else {
                continue;
            };
            let path = table.child(key);
            if !seen.insert(path.clone()) {
                continue; // only the first of several alternative examples is used
            }

            let enabled = enable_example(config, path.clone())
                .unwrap_or_else(|| panic!("{path:?} should get enabled from line {i}"));
            let doc = Document::parse(enabled.as_str()).expect("the config should still parse");
            let set =
                path.0.iter().try_fold(doc.as_item(), |item, name| item.as_table_like()?.get(name));
            assert!(set.is_some(), "{path:?} should be set after enabling line {i}");

            let after: Vec<&str> = enabled.lines().collect();
            let changed = lines.iter().zip(&after).position(|(a, b)| a != b).unwrap_or(lines.len());
            if after.len() == lines.len() {
                assert_eq!(changed, i, "{path:?} should be uncommented on line {i}");
                assert_eq!(format!("# {}", after[i]), lines[i]);
                assert_eq!(after[i + 1..], lines[i + 1..]);
                uncommented += 1;
            } else {
                assert!(
                    changed > i,
                    "{path:?} should be inserted after line {i}, not at {changed}"
                );
                assert!(
                    lines[i..changed]
                        .iter()
                        .all(|line| matches!(line.trim_start().chars().next(), Some('#') | None)),
                    "there should be no non-blank, non-comment lines between the example {path:?} \
                     at {i} and the insertion at {changed}"
                );
                assert_eq!(after[changed], format!("{key} = 0"));
                assert_eq!(after[changed + 1..], lines[changed..]);
            }
        }

        assert!(uncommented > 50, "only {uncommented} examples uncommented in the default config");
    }

    /// The first `ui.columns` example is the default value, and is followed by more, so none of
    /// them is uncommented: the key goes after the last.
    #[rstest]
    fn inserts_ui_columns_after_its_examples_in_the_default_config() {
        let config = Settings::example_config();
        let last_example = config.rfind("\n# columns = ").expect("an example of ui.columns") + 1;
        let end = last_example + config[last_example..].find('\n').expect("a whole line") + 1;

        assert_eq!(
            enable_example(config, TomlPath::parse_dotted_simple("ui.columns")),
            Some(format!("{}columns = 0\n{}", &config[..end], &config[end..]))
        );
    }
}
