//! SSE 只读旁路：累积首轮 assistant 文本/工具调用，流结束提交绑定。
//!
//! 不修改字节、不改变背压；解析失败只记 debug，不影响正常转发。

use std::collections::BTreeMap;

use axum::body::Bytes;
use futures_util::{stream, Stream, StreamExt};
use serde_json::Value;

use super::extract::{assistant_hash, canonical_tool_calls, normalize_text};
use super::SessionFallbackCapture;

/// 累积 OpenAI Chat Completions 流式 delta。
#[derive(Default)]
pub(crate) struct AssistantAccumulator {
    text: String,
    tool_calls: BTreeMap<i64, ToolCallAccum>,
}

#[derive(Default)]
struct ToolCallAccum {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

impl AssistantAccumulator {
    pub(crate) fn consume_data(&mut self, data: &str) {
        if data.trim() == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            tracing::debug!("session fallback: skipped non-JSON SSE data");
            return;
        };
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        let Some(delta) = choice.get("delta") else {
            return;
        };
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(content);
        }
        let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) else {
            return;
        };
        for (fallback_index, call) in tool_calls.iter().enumerate() {
            let index = call
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(fallback_index as i64);
            let entry = self.tool_calls.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(kind) = call.get("type").and_then(Value::as_str) {
                if !kind.is_empty() {
                    entry.kind = kind.to_string();
                }
            }
            let Some(function) = call.get("function") else {
                continue;
            };
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                if !name.is_empty() {
                    entry.name = name.to_string();
                }
            }
            if let Some(arguments) = function.get("arguments") {
                match arguments {
                    Value::String(fragment) => entry.arguments.push_str(fragment),
                    other => {
                        entry.arguments = serde_json::to_string(other).unwrap_or_default();
                    }
                }
            }
        }
    }

    /// 返回规范化 assistant 内容；正文优先，其次 tool_calls。
    pub(crate) fn finish(&mut self) -> Option<String> {
        let text = normalize_text(&std::mem::take(&mut self.text));
        if !text.is_empty() {
            return Some(text);
        }
        if self.tool_calls.is_empty() {
            return None;
        }
        let calls: Vec<Value> = std::mem::take(&mut self.tool_calls)
            .into_iter()
            .map(|(index, call)| {
                serde_json::json!({
                    "index": index,
                    "id": call.id,
                    "type": if call.kind.is_empty() { "function".to_string() } else { call.kind },
                    "function": { "name": call.name, "arguments": call.arguments },
                })
            })
            .collect();
        canonical_tool_calls(&calls)
    }
}

/// 包装上游字节流：原样转发每个 chunk，同时旁路累积 assistant 内容。
pub(crate) fn wrap_stream<S, E>(
    inner: S,
    capture: SessionFallbackCapture,
) -> stream::BoxStream<'static, Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: Send + 'static,
{
    struct State<S> {
        inner: S,
        parser: crate::proxy::sse::SseEventParser,
        accumulator: AssistantAccumulator,
        capture: Option<SessionFallbackCapture>,
        finished: bool,
    }

    fn finish<S>(state: &mut State<S>) {
        if state.finished {
            return;
        }
        state.finished = true;
        let mut events = Vec::new();
        state.parser.finish(|data| events.push(data));
        for data in events {
            state.accumulator.consume_data(&data);
        }
        let Some(capture) = state.capture.take() else {
            return;
        };
        let Some(canonical) = state.accumulator.finish() else {
            tracing::debug!(
                upstream_id = capture.upstream_id(),
                "session fallback: no assistant content in stream; binding deferred"
            );
            return;
        };
        capture.commit(&assistant_hash(&canonical));
    }

    Box::pin(stream::unfold(
        State {
            inner,
            parser: crate::proxy::sse::SseEventParser::new(),
            accumulator: AssistantAccumulator::default(),
            capture: Some(capture),
            finished: false,
        },
        |mut state| async move {
            if state.finished {
                return None;
            }
            match state.inner.next().await {
                Some(Ok(chunk)) => {
                    let mut saw_done = false;
                    let State {
                        parser, accumulator, ..
                    } = &mut state;
                    parser.push_chunk(&chunk, |data| {
                        if data.trim() == "[DONE]" {
                            saw_done = true;
                        }
                        accumulator.consume_data(&data);
                    });
                    if saw_done {
                        // 下游 SSE 变换可能在 [DONE] 处提前结束，不再轮询 EOF；
                        // 在这里提交，保证首轮绑定不依赖外层是否读到流末尾。
                        finish(&mut state);
                    }
                    Some((Ok(chunk), state))
                }
                Some(Err(error)) => {
                    finish(&mut state);
                    Some((Err(error), state))
                }
                None => {
                    finish(&mut state);
                    None
                }
            }
        },
    ))
}
