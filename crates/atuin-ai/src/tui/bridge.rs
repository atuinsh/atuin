//! The chat stream as a message stream.
//!
//! v1's `run_stream_bridge` was an async task threaded with a
//! `watch`-channel cancellation protocol (checked before the context
//! gather and on every frame). Here the bridge is a plain
//! `Stream<Item = Msg>` driven by `ctx.spawn`: dropping the app's `Task`
//! drops this generator at whatever await point it's suspended on, which
//! aborts the HTTP stream.

use std::sync::Arc;

use atuin_client::settings::Settings;
use futures::StreamExt;

use crate::context::{AppContext, ClientContext};
use crate::fsm::events::Event;
use crate::skills::SkillSummary;
use crate::stream::{ChatRequest, StreamContent, StreamControl, StreamFrame, create_chat_stream};
use crate::tui::app::Msg;

pub fn stream_bridge(
    request: ChatRequest,
    app_ctx: AppContext,
    client_ctx: ClientContext,
    skill_summaries: Vec<SkillSummary>,
    skill_overflow: Option<String>,
    settings: Arc<Settings>,
) -> impl futures::Stream<Item = Msg> + Send {
    async_stream::stream! {
        let stream = create_chat_stream(
            app_ctx.endpoint.clone(),
            app_ctx.token.clone(),
            app_ctx.token_from_hub_session,
            request,
            client_ctx,
            app_ctx.send_cwd,
            app_ctx.last_command.clone(),
            skill_summaries,
            skill_overflow,
            settings,
        );
        futures::pin_mut!(stream);

        yield Msg::Fsm(Event::StreamStarted);

        while let Some(frame) = stream.next().await {
            let event = match frame {
                Ok(StreamFrame::Content(content)) => match content {
                    StreamContent::TextChunk(text) => Some(Event::StreamChunk(text)),
                    StreamContent::ToolCall { id, name, input } => {
                        if name == "suggest_command" {
                            Some(Event::SuggestCommand { id, input })
                        } else {
                            Some(Event::StreamToolCall { id, name, input })
                        }
                    }
                    StreamContent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        remote,
                        content_length,
                    } => Some(Event::StreamServerToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        remote,
                        content_length,
                    }),
                },
                Ok(StreamFrame::Control(control)) => match control {
                    StreamControl::StatusChanged(status) => {
                        Some(Event::StreamStatusChanged(status))
                    }
                    StreamControl::Done {
                        session_id,
                        credits,
                    } => {
                        if let Some(snapshot) = credits {
                            yield Msg::Usage(snapshot);
                        }
                        Some(Event::StreamDone { session_id })
                    }
                    StreamControl::Error(msg) => Some(Event::StreamError(msg)),
                },
                Ok(StreamFrame::SessionIdentity(session_id)) => {
                    Some(Event::SessionIdReceived(session_id))
                }
                Err(e) => Some(Event::StreamError(e.to_string())),
            };

            if let Some(event) = event {
                // StreamDone and StreamError are terminal — the server won't
                // send more. SuggestCommand is NOT terminal: StreamDone
                // follows it with the session_id we need to capture.
                let is_terminal =
                    matches!(event, Event::StreamDone { .. } | Event::StreamError(_));
                yield Msg::Fsm(event);
                if is_terminal {
                    break;
                }
            }
        }
    }
}
