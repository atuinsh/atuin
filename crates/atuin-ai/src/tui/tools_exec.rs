//! Client-side tool execution as spawned message streams and futures.

use tokio::sync::oneshot;

use crate::fsm::events::Event;
use crate::fsm::tools::ToolPreviewData;
use crate::tools::{ShellToolCall, ToolOutcome};
use crate::tui::app::Msg;

/// Run a shell command, yielding preview-line batches as they stream and
/// the outcome when it finishes.
///
/// Spawned **detached**: interrupt is not cancellation. Ctrl+C sends on the
/// interrupt channel, the child is killed, and the stream still delivers
/// `ToolExecutionDone` so the FSM can report the interrupted outcome.
pub(crate) fn shell_stream(
    tool_id: String,
    call: ShellToolCall,
    interrupt_rx: oneshot::Receiver<()>,
) -> impl futures::Stream<Item = Msg> + Send {
    async_stream::stream! {
        let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<Vec<String>>(16);
        let exec = crate::tools::execute_shell_command_streaming(&call, output_tx, interrupt_rx);
        futures::pin_mut!(exec);

        let outcome = loop {
            tokio::select! {
                biased;
                maybe = output_rx.recv() => match maybe {
                    Some(lines) => yield Msg::Fsm(Event::ToolPreviewUpdate {
                        tool_id: tool_id.clone(),
                        lines,
                        exit_code: None,
                    }),
                    // The exec future owns the only sender, so a closed
                    // channel means it's done — finish polling it.
                    None => break (&mut exec).await,
                },
                outcome = &mut exec => break outcome,
            }
        };

        // Output batches that raced the completion.
        while let Ok(lines) = output_rx.try_recv() {
            yield Msg::Fsm(Event::ToolPreviewUpdate {
                tool_id: tool_id.clone(),
                lines,
                exit_code: None,
            });
        }

        let preview = if let ToolOutcome::Structured { exit_code, .. } = &outcome {
            Some(ToolPreviewData::Shell {
                lines: vec![],
                exit_code: *exit_code,
                // Reason is set by the FSM in handle_tool_done based on
                // whether it was a user interrupt or timeout.
                interrupted: None,
            })
        } else {
            None
        };

        yield Msg::Fsm(Event::ToolExecutionDone {
            tool_id,
            outcome,
            preview,
        });
    }
}

/// Load a skill's content, with errors folded into the returned string
/// (they go to the model as conversation content, not failures).
pub(crate) async fn load_skill_content(
    registry: &crate::skills::SkillRegistry,
    name: &str,
    shell: &str,
    arguments: Option<&str>,
) -> String {
    match registry.load(name, shell, arguments).await {
        Ok(body) => body,
        Err(e) => format!("Failed to load skill '{name}': {e}"),
    }
}

// Unix-gated per the repo convention for tests that spawn real shell
// subprocesses: these exercise the stream plumbing (select loop, preview
// batching, interrupt delivery), which is identical Rust on every
// platform — `bash -c` is just the vehicle, and it isn't reliably
// present on Windows runners.
#[cfg(all(test, unix))]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::tools::ShellToolCall;

    fn shell_call(command: &str) -> ShellToolCall {
        ShellToolCall::try_from(&serde_json::json!({ "command": command })).unwrap()
    }

    #[tokio::test]
    async fn shell_stream_yields_previews_then_outcome() {
        let (_interrupt_tx, interrupt_rx) = oneshot::channel();
        let msgs: Vec<Msg> = shell_stream("t1".into(), shell_call("echo hello"), interrupt_rx)
            .collect()
            .await;

        let mut saw_preview_with_output = false;
        let mut done: Option<&Event> = None;
        for msg in &msgs {
            match msg {
                Msg::Fsm(Event::ToolPreviewUpdate { tool_id, lines, .. }) => {
                    assert_eq!(tool_id, "t1");
                    assert!(done.is_none(), "previews must precede the outcome");
                    if lines.iter().any(|l| l.contains("hello")) {
                        saw_preview_with_output = true;
                    }
                }
                Msg::Fsm(e @ Event::ToolExecutionDone { .. }) => done = Some(e),
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert!(
            saw_preview_with_output,
            "expected echoed output in a preview"
        );
        let Some(Event::ToolExecutionDone {
            tool_id, outcome, ..
        }) = done
        else {
            panic!("missing ToolExecutionDone");
        };
        assert_eq!(tool_id, "t1");
        assert!(
            matches!(
                outcome,
                ToolOutcome::Structured {
                    exit_code: Some(0),
                    ..
                }
            ),
            "expected clean exit, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn shell_stream_interrupt_still_delivers_outcome() {
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let stream = shell_stream("t2".into(), shell_call("sleep 30"), interrupt_rx);
        futures::pin_mut!(stream);

        let _ = interrupt_tx.send(());

        // Interrupt is not cancellation: the stream must still end with an
        // outcome (promptly — not after the sleep).
        let all = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut v = Vec::new();
            while let Some(m) = stream.next().await {
                v.push(m);
            }
            v
        })
        .await
        .expect("interrupted stream should finish promptly");

        assert!(
            matches!(all.last(), Some(Msg::Fsm(Event::ToolExecutionDone { .. }))),
            "interrupted execution must still report ToolExecutionDone"
        );
    }
}
