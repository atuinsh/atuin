//! The richer views sessions earn over one-line commands: the detail pane beside the list on wide
//! terminals, activity sparklines, token counts, and the fork/subagent tree.

use atuin_client::ai_session::HarnessSession;
use atuin_client::theme::{Meaning, Theme};
use atuin_common::harnesstools::session::Usage;
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{EllipsizeExt as _, Measure};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::render::{ago, harness_style, highlighted_spans, is_live, repo_name, style};
use super::source::{Relation, SessionRow, harness_label};
use super::state::State;

/// Terminals at least this wide show the detail pane beside the list instead of the preview
/// strip under it.
pub const SPLIT_MIN_WIDTH: u16 = 120;

/// `1234` → `1.2k`, `2_700_000` → `2.7M`.
pub fn human(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}k", f / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", f / 1_000.0)
    } else {
        n.to_string()
    }
}

/// `in 327k · out 58k · cache 2.7M`, or `None` when nothing was reported.
pub fn tokens(usage: &Usage) -> Option<String> {
    let parts: Vec<String> = [
        ("in", usage.input),
        ("out", usage.output),
        ("cache", usage.cache_read.map(|r| r + usage.cache_write.unwrap_or(0))),
    ]
    .into_iter()
    .filter_map(|(label, n)| n.filter(|n| *n > 0).map(|n| format!("{label} {}", human(n))))
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// When the session's messages happened, `width` cells across its lifetime: a bar per bucket,
/// scaled to the busiest, blank where nothing happened.
pub fn sparkline(
    times: &[OffsetDateTime],
    start: OffsetDateTime,
    end: OffsetDateTime,
    width: usize,
) -> String {
    if times.is_empty() || width == 0 {
        return String::new();
    }
    let span = (end - start).whole_seconds().max(1);
    let mut buckets = vec![0u32; width];
    for t in times {
        let at = (*t - start).whole_seconds().clamp(0, span);
        let i = usize::try_from(at * i64::try_from(width - 1).unwrap_or(0) / span).unwrap_or(0);
        buckets[i.min(width - 1)] += 1;
    }
    let max = buckets.iter().copied().max().unwrap_or(1).max(1);
    buckets
        .iter()
        .map(|&n| {
            if n == 0 {
                ' '
            } else {
                // Any activity shows at least the lowest bar.
                let level = (n * 8).div_ceil(max) as usize - 1;
                BARS[level.min(7)]
            }
        })
        .collect()
}

/// Fit `text` (flattened to one paragraph) into at most `lines` lines of `width` columns.
fn clamp_lines(text: &str, width: usize, lines: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Wrapping breaks at spaces, so leave a little slack per line.
    let budget = (width.saturating_sub(4)) * lines;
    flat.ellipsize(Measure::Columns(budget), Pos::End, Indicator::UNICODE).to_string()
}

impl State {
    /// The detail pane for the selected session: what it is, where and when it ran, how busy it
    /// was, and the conversation's first prompt, match and last reply, wrapped.
    pub fn detail_lines(&self, width: usize, height: usize, theme: &Theme) -> Vec<Line<'static>> {
        let Some(row) = self.selected() else {
            return Vec::new();
        };
        let now = (self.now)();
        let base = style(theme, Meaning::Base);
        let muted = style(theme, Meaning::Annotation);
        let heading = muted.add_modifier(Modifier::BOLD);
        let hl = style(theme, Meaning::AlertWarn).add_modifier(Modifier::BOLD);
        let sep = || Span::styled(" · ", muted);

        let mut lines = vec![Line::from(Span::styled(
            clamp_lines(&row.title.text, width, 2),
            base.add_modifier(Modifier::BOLD),
        ))];

        let mut who = vec![Span::styled(
            harness_label(row.handle.harness),
            harness_style(theme, row.handle.harness),
        )];
        if let Some(model) = &row.model {
            who.extend([sep(), Span::styled(model.clone(), muted)]);
        }
        lines.push(Line::from(who));

        let mut place = vec![Span::styled(repo_name(row), muted)];
        if let Some(branch) = &row.branch {
            place.extend([sep(), Span::styled(branch.clone(), style(theme, Meaning::Guidance))]);
        }
        place.extend([sep(), Span::styled(format!("@{}", row.hostname), muted)]);
        lines.push(Line::from(place));

        let mut when = Vec::new();
        if is_live(now, row) {
            when.extend([Span::styled("● live", style(theme, Meaning::AlertInfo)), sep()]);
        }
        when.push(Span::styled(
            format!(
                "{} messages{}",
                row.message_count,
                if row.children > 0 {
                    format!(" · +{} grouped", row.children)
                } else {
                    String::new()
                }
            ),
            muted,
        ));
        when.extend([
            sep(),
            Span::styled(format!("started {} ago", ago(now, row.started_at)), muted),
        ]);
        lines.push(Line::from(when));

        let preview = self.previews.get(&row.handle);
        if let Some(p) = preview.filter(|p| !p.activity.is_empty()) {
            let spark = sparkline(&p.activity, row.started_at, row.updated_at, width);
            lines.push(Line::from(Span::styled(spark, style(theme, Meaning::Guidance))));
        }
        if let Some(t) = tokens(&row.usage) {
            lines.push(Line::from(Span::styled(format!("{t} tokens"), muted)));
        }

        // The conversation, in the space that's left: the last reply gets whatever the first
        // prompt and the match don't use.
        let left = height.saturating_sub(lines.len());
        let section =
            |lines: &mut Vec<Line<'static>>, title: &'static str, spans: Vec<Span<'static>>| {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(title, heading)));
                lines.push(Line::from(spans));
            };
        let Some(p) = preview else {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("…", muted)));
            return lines;
        };
        let parts_budget = left.saturating_sub(6);
        if let Some(first) = &p.first_prompt {
            let text = clamp_lines(first, width, (parts_budget / 3).max(1));
            section(&mut lines, "First prompt", vec![Span::styled(text, base)]);
        }
        if let Some(m) = &row.matched {
            // Highlights index the unclamped text, so show the match whole; it is short.
            section(&mut lines, "Match", highlighted_spans(&m.text, &m.highlights, base, hl));
        }
        if let Some(last) = &p.last_assistant {
            let used = lines.len();
            let rest = height.saturating_sub(used + 2).max(1);
            section(&mut lines, "Last reply", vec![Span::styled(
                clamp_lines(last, width, rest),
                muted,
            )]);
        }
        lines
    }
}

/// The sessions grouped under `root`, as a tree by parent: forks under what they forked,
/// subagents under what spawned them.
pub fn tree_lines(
    root: &HarnessSession,
    children: &[SessionRow],
    now: OffsetDateTime,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    fn walk(
        parent: &HarnessSession,
        children: &[SessionRow],
        prefix: &str,
        out: &mut Vec<(String, usize)>,
        seen: &mut Vec<usize>,
    ) {
        let kids: Vec<usize> = (0..children.len())
            .filter(|&i| !seen.contains(&i) && children[i].parent.as_ref() == Some(parent))
            .collect();
        for (n, &i) in kids.iter().enumerate() {
            seen.push(i);
            let last = n + 1 == kids.len();
            out.push((
                format!(
                    "{prefix}{}",
                    if last {
                        "└─ "
                    } else {
                        "├─ "
                    }
                ),
                i,
            ));
            let deeper = format!(
                "{prefix}{}",
                if last {
                    "   "
                } else {
                    "│  "
                }
            );
            walk(&children[i].handle, children, &deeper, out, seen);
        }
    }

    let mut order = Vec::new();
    let mut seen = Vec::new();
    walk(root, children, "   ", &mut order, &mut seen);
    // Anything whose parent isn't here (not stored yet) hangs off the root.
    for i in 0..children.len() {
        if !seen.contains(&i) {
            order.push(("   ├─ ".to_owned(), i));
        }
    }

    let muted = style(theme, Meaning::Annotation);
    let base = style(theme, Meaning::Base);
    order
        .into_iter()
        .map(|(branch, i)| {
            let child = &children[i];
            let (tag, meaning) = match child.relation {
                Relation::Fork => ("fork", Meaning::Guidance),
                Relation::Subagent => ("subagent", Meaning::Important),
                Relation::Child => ("child", Meaning::Guidance),
                Relation::Root => ("session", Meaning::Base),
            };
            let tail = format!(
                "{:>5} msgs  {:>9}",
                child.message_count,
                format!("{} ago", ago(now, child.updated_at))
            );
            let lead = branch.width() + 10;
            let title_w = width.saturating_sub(lead + 2 + tail.width());
            let mut title = child.title.text.split_whitespace().collect::<Vec<_>>().join(" ");
            // A continuation in another harness (`atuin ai resume --in`) says where it went on.
            if child.parent.as_ref().is_some_and(|p| p.harness != child.handle.harness) {
                title = format!("continued in {} · {title}", harness_label(child.handle.harness));
            }
            let title = title
                .pad_ellipsize(
                    Measure::Columns(title_w),
                    Pos::End,
                    Indicator::UNICODE,
                    atuin_common::string::Alignment::Start,
                )
                .into_owned();
            Line::from(vec![
                Span::styled(branch, muted),
                Span::styled(format!("{tag:<10}"), style(theme, meaning)),
                Span::styled(title, base),
                Span::raw("  "),
                Span::styled(tail, muted),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::Duration;

    use super::*;

    #[rstest]
    #[case(0, "0")]
    #[case(999, "999")]
    #[case(1_234, "1.2k")]
    #[case(58_300, "58k")]
    #[case(2_712_000, "2.7M")]
    fn human_counts(#[case] n: u64, #[case] want: &str) {
        assert_eq!(human(n), want);
    }

    #[rstest]
    fn tokens_skip_what_was_not_reported() {
        assert_eq!(tokens(&Usage::default()), None);
        let usage = Usage {
            input: Some(327_000),
            output: Some(58_000),
            cache_read: Some(2_500_000),
            cache_write: Some(200_000),
            reasoning: None,
        };
        assert_eq!(tokens(&usage).unwrap(), "in 327k · out 58k · cache 2.7M");
    }

    #[rstest]
    fn sparkline_scales_to_the_busiest_bucket() {
        let start = OffsetDateTime::UNIX_EPOCH;
        let end = start + Duration::seconds(90);
        let mut times = vec![start; 8];
        times.push(end);
        let spark = sparkline(&times, start, end, 10);
        assert_eq!(spark.chars().count(), 10);
        assert_eq!(spark.chars().next(), Some('█'));
        assert_eq!(spark.chars().last(), Some('▁'));
        assert!(spark.chars().skip(1).take(8).all(|c| c == ' '));
        assert_eq!(sparkline(&[], start, end, 10), "");
    }
}
