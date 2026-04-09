# Plan: Add Table Rendering to atuin-ai Markdown

## Context

The `Markdown` component (`crates/atuin-ai/src/tui/components/markdown.rs`) renders AI
responses using pulldown-cmark, but drops table events silently (`_ => {}` at line 209).
pulldown-cmark supports GFM tables via `Options::ENABLE_TABLES`. The goal is to render
markdown tables as proper terminal tables.

## Architecture: Composite `#[component]` returning stacked Elements

Convert `Markdown` from a `#[props]` + manual `impl Component` leaf to a `#[component]`
function returning `Elements`. Each markdown block becomes a child element:

- **Text blocks** → `Canvas` leaf wrapping `Paragraph` (with width-aware
  `desired_height_fn` — no probe render needed)
- **Table blocks** → `View` containing stacked `HStack` rows, each with `Column`
  children for cells. `WidthConstraint::Fill` gives equal column widths; the framework's
  `allocate_widths` handles distribution.

This matches the existing pattern used by eye_declare's built-in `Markdown` component
(`eye_declare/src/components/markdown.rs:99-118`) and the `Spinner` component.

Why this approach over a leaf component:
1. **Streaming**: AI responses stream in, so markdown re-renders frequently. The
   `#[component]` function runs during reconciliation (fast parse), while rendering is
   deferred and cached per-node. Canvas reports height via `desired_height_fn` — no
   probe render fallback.
2. **Accurate heights**: Table rows are composites — the framework measures children
   automatically. No manual height estimation.
3. **Natural layout**: `HStack` + `Column` with `Fill` widths gives equal columns for
   free. Can later measure content for `Fixed(n)` widths.

## Files to modify

1. **`crates/atuin-ai/src/tui/components/markdown.rs`** — all changes
2. No dependency or Cargo.toml changes needed

## Implementation

### Step 1: Define block types

Add an internal enum for parsed markdown blocks:

```rust
enum MarkdownBlock {
    Text(Vec<Line<'static>>),
    Table {
        headers: Vec<Vec<Span<'static>>>,
        rows: Vec<Vec<Vec<Span<'static>>>>,  // row → cell → spans
        alignments: Vec<pulldown_cmark::Alignment>,
    },
}
```

### Step 2: Refactor `parse_markdown` → `parse_markdown_blocks`

Returns `Vec<MarkdownBlock>` instead of `Text<'static>`.

Enable table parsing: `Parser::new_ext(source, Options::ENABLE_TABLES)`. Add `Options` to
the `pulldown_cmark` import.

The existing event loop handles inline formatting (bold, italic, code) via the style
stack — this works identically inside table cells. New logic:

- Track `in_table`, `in_table_head`, `in_table_row`, `in_table_cell` bools
- Accumulate cell content as `Vec<Span<'static>>` per cell
- On `Event::End(TagEnd::TableCell)`: push cell to current row vec
- On `Event::End(TagEnd::TableRow)`: push row to table rows vec
- On `Event::End(TagEnd::Table)`: emit `MarkdownBlock::Table`, reset table state
- At block boundaries (paragraph, heading, code block, list) outside tables: flush
  accumulated lines as `MarkdownBlock::Text`

pulldown-cmark table event structure:
```
Start(Table(Vec<Alignment>))
  Start(TableHead)
    Start(TableRow)
      Start(TableCell)  Text(...)  End(TableCell)
    End(TableRow)
  End(TableHead)
  Start(TableRow)           // body rows, no TableBody wrapper
    Start(TableCell)  Text(...)  End(TableCell)
  End(TableRow)
End(Table)
```

### Step 3: Convert to `#[component]` function

Keep `#[props]` on the struct (generates builder for `element!` macro). Remove the manual
`impl Component for Markdown` block. Add a `#[component]` function:

```rust
#[eye_declare_macros::component(
    props = Markdown,
    state = MarkdownStyles,
    initial_state = MarkdownStyles::new(),
    crate_path = eye_declare,
)]
fn markdown(props: &Markdown, styles: &MarkdownStyles) -> Elements {
    if props.source.is_empty() {
        return Elements::new();
    }
    let blocks = parse_markdown_blocks(&props.source, styles);
    let mut els = Elements::new();
    for block in blocks {
        match block {
            MarkdownBlock::Text(lines) => {
                // Canvas with Paragraph — same pattern as eye_declare's built-in Markdown
                let text = Text::from(lines);
                let text_for_height = text.clone();
                els.add(
                    Canvas::builder()
                        .render_fn(move |area: Rect, buf: &mut Buffer| {
                            Paragraph::new(text.clone())
                                .wrap(Wrap { trim: false })
                                .render(area, buf);
                        })
                        .desired_height_fn(move |width: u16| {
                            Paragraph::new(text_for_height.clone())
                                .wrap(Wrap { trim: false })
                                .line_count(width) as u16
                        })
                        .build(),
                );
            }
            MarkdownBlock::Table { headers, rows, .. } => {
                els.add(table_element(headers, rows));
            }
        }
    }
    els
}
```

### Step 4: Build table elements with HStack/Column

The `table_element` function builds a `View` child tree:

```rust
fn table_element(
    headers: Vec<Vec<Span<'static>>>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
) -> Elements {
    let num_cols = headers.len().max(rows.first().map_or(0, |r| r.len()));

    element! {
        View(padding_top: Cells::from(1), padding_bottom: Cells::from(1)) {
            // Header row
            HStack {
                #(for header_cell in &headers {
                    Column {
                        Text {
                            #(for span in header_cell.clone() {
                                Span(
                                    text: span.content.to_string(),
                                    style: span.style.add_modifier(Modifier::BOLD),
                                )
                            })
                        }
                    }
                })
                // Pad remaining columns if headers < num_cols
                #(for _ in headers.len()..num_cols {
                    Column { Text { "" } }
                })
            }
            // Data rows
            #(for row in &rows {
                HStack {
                    #(for cell_spans in row {
                        Column {
                            Text {
                                #(for span in cell_spans.clone() {
                                    Span(
                                        text: span.content.to_string(),
                                        style: span.style,
                                    )
                                })
                            }
                        }
                    })
                    #(for _ in row.len()..num_cols {
                        Column { Text { "" } }
                    })
                }
            })
        }
    }
}
```

Each `Column` defaults to `WidthConstraint::Fill`, so the framework allocates equal width
per column. The `View` wrapper adds vertical padding around the table.

### Step 5: Update imports

```rust
use eye_declare::{
    Cells, Canvas, Column, Component, Elements, HStack, Span, Text, View, element, props,
};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
```

Remove: `use ratatui_core::widgets::Widget;` (no longer rendering directly; Canvas and
the framework handle it).

### Step 6: Remove old code

Delete:
- `impl Component for Markdown { ... }` block (lines 63–91)
- `fn parse_markdown(...)` (lines 94–215) — replaced by `parse_markdown_blocks`
- `MarkdownStyles::initial_state()` on the Component impl (handled by `#[component]`)

## Edge cases

- **Uneven rows**: Pad shorter rows with empty `Column` children (shown above)
- **Empty tables**: Skip (don't emit element)
- **Column spacing**: Start without spacer columns (content provides natural whitespace).
  Add `Fixed(1)` spacer Columns later if too cramped.
- **Inline formatting in cells**: Handled by existing style stack during parsing — bold,
  italic, code inside cells all work
- **Wide tables**: Columns compress equally; content may truncate (acceptable for
  terminal). Content-based column sizing is a future enhancement.

## Verification

```sh
cargo build -p atuin-ai
cargo clippy -p atuin-ai -- -D warnings
cargo test -p atuin-ai
cargo fmt --check
```

Add unit tests:
- Parse a table markdown string and verify block structure
- Render a table into a buffer and verify cell content at expected positions

## Future enhancements (out of scope)

- **Content-based column sizing**: Measure max cell width per column, use
  `WidthConstraint::Fixed(n)` for tighter layout
- **Column alignment**: Map `pulldown_cmark::Alignment::{Left, Center, Right}` to text
  alignment
- **Strikethrough/blockquote**: Other pulldown-cmark extensions
- **Column spacer**: `Fixed(1)` spacer Columns between data columns for visual separation
