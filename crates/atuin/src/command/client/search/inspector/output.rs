use atuin_client::history::HistoryId;
use atuin_client::settings::Settings;
use ratatui::text::{Line, Span, Text};
use tokio::task::JoinHandle;

type LoadedCapture = Result<(Output, String), &'static str>;
const UNAVAILABLE: &str = "Output unavailable — could not read from the daemon";

/// A cancellable background fetch. Completed captures are immutable and reusable;
/// missing/unavailable captures are retried only when the reader is reopened.
#[derive(Default)]
pub(super) struct Capture {
    id: Option<HistoryId>,
    task: Option<JoinHandle<LoadedCapture>>,
    ready: Option<LoadedCapture>,
}

impl Capture {
    pub fn reopen(&mut self) {
        if matches!(self.ready, Some(Err(_))) {
            self.id = None;
        }
    }

    pub async fn poll(&mut self, id: HistoryId, settings: &Settings) -> bool {
        let changed = self.id != Some(id);
        if changed {
            if let Some(task) = self.task.take() {
                task.abort();
            }

            self.id = Some(id);
            self.ready = None;
            let settings = settings.clone();
            self.task = Some(tokio::spawn(async move { load_output(id, &settings).await }));
        }

        // Never await a pending request in the input loop: Esc must remain responsive.
        if self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            self.ready = Some(self.task.take().unwrap().await.unwrap_or(Err(UNAVAILABLE)));
        }

        changed
    }

    pub fn status(&self) -> &str {
        match &self.ready {
            Some(Ok((_, status))) => status,
            Some(Err(message)) => message,
            None => "Loading output…",
        }
    }

    pub fn rows(&mut self, width: u16) -> &[Line<'static>] {
        match &mut self.ready {
            Some(Ok((output, _))) => output.rows(width),
            _ => &[],
        }
    }

    #[cfg(test)]
    pub fn from_text(text: &str, status: &str) -> Self {
        Self {
            ready: Some(Ok((Output::parse(text), status.into()))),
            id: None,
            task: None,
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[cfg(feature = "daemon")]
async fn load_output(id: HistoryId, settings: &Settings) -> LoadedCapture {
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut client = atuin_daemon::HistoryClient::from_settings(settings).await?;
        client.get_command_output(id, vec![atuin_common::range::PyStyleIdxRange::new(0, -1)]).await
    })
    .await;

    match result {
        Ok(Ok(Some(output))) => {
            let text = Output::parse(
                &output
                    .chunks
                    .iter()
                    .map(|chunk| chunk.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );

            let status = if output.meta.as_ref().is_some_and(|meta| meta.output_truncated) {
                "Output · capture truncated"
            } else if text.is_empty() {
                "Captured output is empty"
            } else {
                "Output"
            };

            Ok((text, status.into()))
        }
        Ok(Ok(None)) => Err("No output captured for this run"),
        _ => Err(UNAVAILABLE),
    }
}

#[cfg(not(feature = "daemon"))]
async fn load_output(_id: HistoryId, _settings: &Settings) -> LoadedCapture {
    Err("Output requires a build with daemon support")
}

/// Only parsed text and styles reach the renderer; never the original escape sequences.
#[derive(Default)]
pub(super) struct Output {
    text: Text<'static>,
    width: u16,
    rows: Vec<Line<'static>>,
}

impl Output {
    #[cfg(any(feature = "daemon", test))]
    pub fn parse(capture: &str) -> Self {
        use ansi_to_tui::IntoText as _;
        use ratatui::style::Modifier;

        let mut text = match capture.into_text() {
            Ok(mut text) => {
                // The converter consumes the final newline; preserve the trailing row.
                if capture.ends_with('\n') {
                    text.lines.push(Line::default());
                }
                text
            }
            Err(_) => Text::raw(plain_output(capture)),
        };

        // Captures should contain only SGR and newlines. Defensively remove any other
        // controls left by the converter, and don't allow output to blink or conceal text.
        for span in text.lines.iter_mut().flat_map(|line| &mut line.spans) {
            span.content.to_mut().retain(|c| !c.is_control());
            span.style = span
                .style
                .remove_modifier(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK | Modifier::HIDDEN);
        }

        if text.lines.len() == 1 && text.lines[0].spans.iter().all(|s| s.content.is_empty()) {
            text.lines.clear();
        }

        Self {
            text,
            ..Self::default()
        }
    }

    #[cfg(feature = "daemon")]
    pub fn is_empty(&self) -> bool {
        self.text.lines.is_empty()
    }

    pub fn rows(&mut self, width: u16) -> &[Line<'static>] {
        let width = width.max(1);
        if self.width != width {
            self.rows.clear();
            for line in &self.text.lines {
                // Reuse capture wrapping (including whitespace and wide characters),
                // then split the already-parsed spans at those same byte boundaries.
                let plain = line.to_string();
                let mut spans = line
                    .spans
                    .iter()
                    .filter(|s| !s.content.is_empty())
                    .map(|s| (s.content.as_ref(), s.style));
                let mut span = spans.next();

                for row in vt100::capture::basic_formatted_rows(&plain, width) {
                    let mut remaining = row.len();
                    let mut wrapped = Line::default();

                    while let Some((content, style)) = span
                        && remaining > 0
                    {
                        let end = remaining.min(content.len());
                        wrapped.spans.push(Span::styled(content[..end].to_owned(), style));
                        remaining -= end;
                        span = if end == content.len() {
                            spans.next()
                        } else {
                            Some((&content[end..], style))
                        };
                    }

                    self.rows.push(wrapped);
                }
            }
            self.width = width;
        }

        &self.rows
    }
}

#[cfg(any(feature = "daemon", test))]
fn plain_output(output: &str) -> String {
    vt100::capture::basic_formatted_to_plain(output)
        .collect::<String>()
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier};
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(true)]
    #[case(false)]
    #[tokio::test]
    async fn loading_is_nonblocking_and_only_failures_retry(#[case] success: bool) {
        let id = HistoryId::new(atuin_common::utils::uuid_v7());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut capture = Capture {
            id: Some(id),
            task: Some(tokio::spawn(async move { receiver.await.unwrap() })),
            ready: None,
        };
        let settings = Settings::utc();
        assert!(
            !tokio::time::timeout(
                std::time::Duration::from_millis(100),
                capture.poll(id, &settings)
            )
            .await
            .unwrap()
        );
        assert_eq!(capture.status(), "Loading output…");
        assert!(capture.rows(80).is_empty());
        let result = if success {
            Ok((Output::parse("\x1b[31mred"), "Output".into()))
        } else {
            Err(UNAVAILABLE)
        };
        assert!(sender.send(result).is_ok());
        tokio::task::yield_now().await;
        assert!(!capture.poll(id, &settings).await);
        assert_eq!(
            capture.status(),
            if success {
                "Output"
            } else {
                UNAVAILABLE
            }
        );
        capture.reopen();
        assert_eq!(capture.id, success.then_some(id));
        if success {
            assert!(!capture.poll(id, &settings).await);
            assert!(capture.task.is_none(), "reopening must not fetch again");
            assert_eq!(capture.rows(80)[0].spans[0].style.fg, Some(Color::Red));
        }
    }

    #[rstest]
    #[case(false)]
    #[case(true)]
    #[tokio::test]
    async fn dropping_or_replacing_a_capture_cancels_the_request(#[case] replace: bool) {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let mut capture = Capture {
            id: Some(HistoryId::new(atuin_common::utils::uuid_v7())),
            task: Some(tokio::spawn(async move {
                let _sender = sender;
                std::future::pending::<LoadedCapture>().await
            })),
            ready: None,
        };
        tokio::task::yield_now().await;
        if replace {
            assert!(
                capture
                    .poll(HistoryId::new(atuin_common::utils::uuid_v7()), &Settings::utc())
                    .await
            );
        }
        drop(capture);
        assert!(receiver.await.is_err());
    }

    #[rstest]
    fn formatting_only() {
        let mut output = Output::parse("\x1b[1;2;3;4;5;6;7;8;9mvisible");
        let span = &output.rows(80)[0].spans[0];
        assert_eq!(span.content, "visible");
        assert_eq!(
            span.style.add_modifier,
            Modifier::BOLD
                | Modifier::DIM
                | Modifier::ITALIC
                | Modifier::UNDERLINED
                | Modifier::REVERSED
                | Modifier::CROSSED_OUT
        );
    }

    #[rstest]
    #[case("before\x1b[2J\x1b[H\x1b[?1049h\x1b[?25lafter", "beforeafter")]
    #[case("before\x1b]52;c;c2VjcmV0\x07after", "beforeafter")]
    #[case("before\x1b]0;title\x07after", "beforeafter")]
    #[case("one\x07\x00\x08\x7f\u{009b}two", "onetwo")]
    #[case("before\x1b[", "before")]
    #[case("before\x1b[38;2;", "before")]
    fn non_formatting_controls_are_not_rendered(#[case] input: &str, #[case] expected: &str) {
        let mut output = Output::parse(input);
        let rows = output.rows(80);
        assert_eq!(rows.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"), expected);
        assert!(
            rows.iter().flat_map(|l| &l.spans).all(|s| !s.content.chars().any(char::is_control))
        );
    }

    #[rstest]
    fn styles_survive_wrapping_newlines_and_resize() {
        let mut output = Output::parse("\x1b[31mabcde\nfgh\x1b[0mi");
        let rows = output.rows(3);
        assert_eq!(rows.iter().map(ToString::to_string).collect::<Vec<_>>(), [
            "abc", "de", "fgh", "i"
        ]);
        for row in &rows[..3] {
            assert_eq!(row.spans[0].style.fg, Some(Color::Red));
        }
        assert_eq!(rows[3].spans[0].style.fg, Some(Color::Reset));
        let rows = output.rows(80);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].spans[0].content, "fgh");
        assert_eq!(rows[1].spans[0].style.fg, Some(Color::Red));
        assert_eq!(rows[1].spans[1].content, "i");
        assert_eq!(rows[1].spans[1].style.fg, Some(Color::Reset));
    }

    #[rstest]
    fn styled_wrapping_preserves_text() {
        use proptest::prelude::*;

        proptest!(|(before in "[a-z 彩色é\u{301}\n]{0,80}", after in "[a-z 彩色é\u{301}\n]{0,80}", width in 1u16..80)| {
            let capture = format!("\x1b[31m{before}\x1b[0m{after}");
            let plain = format!("{before}{after}");
            let mut output = Output::parse(&capture);
            let expected: Vec<_> = if plain.is_empty() { Vec::new() } else {
                vt100::capture::basic_formatted_rows(&plain, width).collect()
            };
            prop_assert_eq!(output.rows(width).iter().map(ToString::to_string).collect::<Vec<_>>(), expected);
        });
    }
}
