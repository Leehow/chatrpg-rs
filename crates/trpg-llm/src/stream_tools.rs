use crate::sse::{SseDecoder, SseItem};
use anyhow::{anyhow, Result};
use async_stream::try_stream;
use futures_core::Stream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::pin::Pin;

/// 一次完整聚合后的 tool call（SSE 分片按 choices[0].delta.tool_calls[].index
/// 聚合、arguments 字符串跨 chunk 拼接而成）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregatedToolCall {
    /// OpenAI tool_call id（回填 tool role message 时必须原样使用）。
    pub id: String,
    /// function name。
    pub name: String,
    /// function arguments 的完整 JSON 字符串（聚合后的原文，由 dispatch 解析）。
    pub arguments: String,
}

/// stream_chat_with_tools 的流事件。一轮流式调用的合法事件序列：
///   工具轮:  ToolCalls(..) → [Usage(..)] → Done{finish_reason: Some("tool_calls")}
///   叙事轮:  ContentDelta.. × N → [Usage(..)] → Done{finish_reason: Some("stop")}
///   混合轮:  ContentDelta.. → ToolCalls(..) → Done{..}（少见但合法，不串台）
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// content 增量（最终叙事的 token 级片段，即到即发，不缓冲）。
    ContentDelta(String),
    /// 全部 tool_calls 分片聚合完成后一次性发出（每轮至多一个该事件）。
    ToolCalls(Vec<AggregatedToolCall>),
    /// 上游 usage 对象原样透传（fail-open，仅观测）。
    Usage(Value),
    /// 流结束；finish_reason 原样透传（"stop" / "tool_calls" / None）。
    Done { finish_reason: Option<String> },
}

/// 末轮强制散文用 tool_choice 控制（§6 末轮强制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    None,
}

impl ToolChoice {
    pub fn as_json(&self) -> Value {
        match self {
            ToolChoice::Auto => Value::String("auto".to_string()),
            ToolChoice::None => Value::String("none".to_string()),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// PURE：SSE chunk 解析/聚合器，与 reqwest 解耦（单测直接喂 JSON chunk）。
#[derive(Debug, Default)]
pub struct ToolStreamAggregator {
    calls: BTreeMap<u64, PartialToolCall>,
    /// finish_reason chunk 只暂存不发 Done：include_usage 的 usage 尾 chunk
    /// 在 finish_reason 之后、[DONE] 之前到达，提前置 done_emitted 会把它吞掉。
    pending_finish_reason: Option<String>,
    done_emitted: bool,
}

impl ToolStreamAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一个已解析的 SSE data JSON（整个 chunk Value，内部取 choices[0].delta）。
    /// content delta 即来即发；tool_calls 分片累积、到 finish_reason chunk 才
    /// 一次性 flush；Done 不在此发出（见 pending_finish_reason 注释与 finish()）。
    pub fn feed_chunk(&mut self, chunk: &Value) -> Vec<StreamEvent> {
        if self.done_emitted {
            return Vec::new();
        }
        let mut out = Vec::new();
        // include_usage 尾 chunk 的 choices 为空数组：usage 检查必须先于 choices 守卫。
        if let Some(usage) = chunk.get("usage") {
            if !usage.is_null() {
                out.push(StreamEvent::Usage(usage.clone()));
            }
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return out;
        };
        if let Some(content) = choice.pointer("/delta/content").and_then(Value::as_str) {
            if !content.is_empty() {
                out.push(StreamEvent::ContentDelta(content.to_string()));
            }
        }
        if let Some(text) = choice.pointer("/text").and_then(Value::as_str) {
            if !text.is_empty() {
                out.push(StreamEvent::ContentDelta(text.to_string()));
            }
        }
        if let Some(parts) = choice
            .pointer("/delta/tool_calls")
            .and_then(Value::as_array)
        {
            for part in parts {
                let index = part.get("index").and_then(Value::as_u64).unwrap_or(0);
                let slot = self.calls.entry(index).or_default();
                if let Some(id) = part.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        slot.id = id.to_string();
                    }
                }
                if let Some(name) = part.pointer("/function/name").and_then(Value::as_str) {
                    if !name.is_empty() {
                        slot.name = name.to_string();
                    }
                }
                if let Some(arguments) = part.pointer("/function/arguments").and_then(Value::as_str)
                {
                    slot.arguments.push_str(arguments);
                }
            }
        }
        // 真实协议序是「finish_reason chunk → usage 尾 chunk → [DONE]」：这里只
        // 暂存 finish_reason 并 flush ToolCalls（分片已齐），Done 统一由 finish()
        // 在 [DONE]/流末发出，保证后到的 usage 尾 chunk 不被 done_emitted 守卫吞掉，
        // 且事件序严格满足契约 ToolCalls → [Usage] → Done。
        if let Some(reason) = choice.get("finish_reason") {
            if !reason.is_null() {
                if let Some(reason) = reason.as_str() {
                    self.pending_finish_reason = Some(reason.to_string());
                }
                out.extend(self.flush_tool_calls());
            }
        }
        out
    }

    /// `[DONE]` 或字节流自然结束时调用：flush 累积的 ToolCalls（若有）+ Done。
    /// finish_reason 显式入参优先；为 None 时回落到 feed_chunk 暂存的协议值。
    pub fn finish(&mut self, finish_reason: Option<String>) -> Vec<StreamEvent> {
        if self.done_emitted {
            return Vec::new();
        }
        self.done_emitted = true;
        let mut out = Vec::new();
        out.extend(self.flush_tool_calls());
        let finish_reason = finish_reason.or_else(|| self.pending_finish_reason.take());
        out.push(StreamEvent::Done { finish_reason });
        out
    }

    /// 把累积完成的 tool_calls 分片一次性聚合发出（空则无事件）。幂等：
    /// mem::take 清空累积，finish_reason chunk 与 finish() 双触发不会重复发。
    fn flush_tool_calls(&mut self) -> Option<StreamEvent> {
        if self.calls.is_empty() {
            return None;
        }
        let calls = std::mem::take(&mut self.calls)
            .into_values()
            .map(|p| AggregatedToolCall {
                id: p.id,
                name: p.name,
                arguments: p.arguments,
            })
            .collect::<Vec<_>>();
        Some(StreamEvent::ToolCalls(calls))
    }
}

impl crate::OpenAiCompatibleClient {
    /// stream_chat_with_tools 的实现主体。放本文件而非 lib.rs：lib.rs 现 332 行，
    /// 主体写进去必超 400 行纪律；子模块可访问父模块私有字段 config/http 与
    /// 私有重试 helper（max_retries / retry_base_delay_ms / is_retryable_status /
    /// retry_after_delay_ms），lib.rs 的 trait impl 一行委托到此。
    pub(crate) async fn stream_chat_with_tools_impl(
        &self,
        messages: Vec<Value>,
        tools: Vec<Value>,
        tool_choice: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let body = json!({
            "model": self.config.model.clone(),
            "messages": messages,
            "tools": tools,
            "tool_choice": tool_choice.as_json(),
            "stream": true,
            "stream_options": {"include_usage": true}
        });
        let mut last_error: Option<anyhow::Error> = None;
        let mut resp_opt = None;
        for attempt in 0..=self.max_retries() {
            let resp = self
                .http
                .post(self.config.endpoint("chat/completions"))
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await;
            match resp {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        resp_opt = Some(resp);
                        break;
                    }
                    let retry_delay_ms = Self::retry_after_delay_ms(
                        &resp,
                        self.retry_base_delay_ms()
                            .saturating_mul(1u64 << attempt.min(5)),
                    );
                    let text = resp.text().await.unwrap_or_default();
                    let err = anyhow!("LLM API error {status}: {text}");
                    if attempt < self.max_retries() && Self::is_retryable_status(status) {
                        tracing::warn!(attempt = attempt + 1, status = %status, retry_delay_ms, "retrying tool streaming request before first event");
                        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(err) => {
                    let retry_delay_ms = self
                        .retry_base_delay_ms()
                        .saturating_mul(1u64 << attempt.min(5));
                    let err = anyhow!(err)
                        .context("LLM tool streaming transport error before first event");
                    if attempt < self.max_retries() {
                        tracing::warn!(attempt = attempt + 1, retry_delay_ms, error = %err, "retrying tool streaming transport error before first event");
                        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        let resp = resp_opt.ok_or_else(|| {
            last_error.unwrap_or_else(|| {
                anyhow!("LLM tool streaming request failed without a recorded error")
            })
        })?;
        let mut bytes = resp.bytes_stream();
        let s = try_stream! {
            // 与 lib.rs::stream_chat 共用增量 SSE 解码器：就地排干 + parse error 计数。
            let mut decoder = SseDecoder::new();
            let mut agg = ToolStreamAggregator::new();
            let mut done = false;
            'outer: while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                decoder.push_bytes(&chunk);
                for item in decoder.drain_items() {
                    match item {
                        SseItem::Data(parsed) => {
                            for event in agg.feed_chunk(&parsed) { yield event; }
                        }
                        SseItem::Done => {
                            for event in agg.finish(None) { yield event; }
                            done = true;
                            break 'outer;
                        }
                    }
                }
            }
            if !done {
                for event in agg.finish(None) { yield event; }
            }
        };
        Ok(Box::pin(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aggregates_tool_arguments_across_chunks() {
        let mut agg = ToolStreamAggregator::new();
        assert_eq!(agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"roll_check","arguments":"{\"tested_"}}]}}]})), Vec::<StreamEvent>::new());
        assert_eq!(agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"parameter\":\"DEX\"}"}}]}}]})), Vec::<StreamEvent>::new());
        let events = agg.finish(Some("tool_calls".to_string()));
        assert_eq!(
            events,
            vec![
                StreamEvent::ToolCalls(vec![AggregatedToolCall {
                    id: "call_1".to_string(),
                    name: "roll_check".to_string(),
                    arguments: "{\"tested_parameter\":\"DEX\"}".to_string()
                }]),
                StreamEvent::Done {
                    finish_reason: Some("tool_calls".to_string())
                },
            ]
        );
    }

    #[test]
    fn aggregates_multiple_tool_calls_by_index() {
        let mut agg = ToolStreamAggregator::new();
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[
            {"index":1,"id":"call_b","type":"function","function":{"name":"retrieve_rules","arguments":"{\"query\":\"ste"}},
            {"index":0,"id":"call_a","type":"function","function":{"name":"get_actor","arguments":"{\"actor_id\":\"pc"}}
        ]}}]}));
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[
            {"index":1,"function":{"arguments":"alth\"}"}},
            {"index":0,"function":{"arguments":".current\"}"}}
        ]}}]}));
        let events = agg.finish(Some("tool_calls".to_string()));
        assert_eq!(
            events[0],
            StreamEvent::ToolCalls(vec![
                AggregatedToolCall {
                    id: "call_a".to_string(),
                    name: "get_actor".to_string(),
                    arguments: "{\"actor_id\":\"pc.current\"}".to_string()
                },
                AggregatedToolCall {
                    id: "call_b".to_string(),
                    name: "retrieve_rules".to_string(),
                    arguments: "{\"query\":\"stealth\"}".to_string()
                },
            ])
        );
    }

    #[test]
    fn content_and_tools_do_not_mix() {
        let mut agg = ToolStreamAggregator::new();
        assert_eq!(
            agg.feed_chunk(&json!({"choices":[{"delta":{"content":"You duck. "}}]})),
            vec![StreamEvent::ContentDelta("You duck. ".to_string())]
        );
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"change_track","arguments":"{\"bucket\":\"tracks\"}"}}]}}]}));
        let events = agg.finish(Some("tool_calls".to_string()));
        assert_eq!(
            events[0],
            StreamEvent::ToolCalls(vec![AggregatedToolCall {
                id: "call_1".to_string(),
                name: "change_track".to_string(),
                arguments: "{\"bucket\":\"tracks\"}".to_string()
            }])
        );
    }

    #[test]
    fn finish_reason_from_chunk_is_preserved() {
        let mut agg = ToolStreamAggregator::new();
        // finish_reason chunk 只暂存不发 Done（usage 尾 chunk 可能还在后面）。
        let events = agg.feed_chunk(&json!({"choices":[{"delta":{},"finish_reason":"stop"}]}));
        assert_eq!(events, Vec::<StreamEvent>::new());
        // [DONE]/流末统一发 Done，透传暂存的 finish_reason。
        assert_eq!(
            agg.finish(None),
            vec![StreamEvent::Done {
                finish_reason: Some("stop".to_string())
            }]
        );
    }

    #[test]
    fn done_flushes_pending_calls() {
        let mut agg = ToolStreamAggregator::new();
        agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"remember","arguments":"{\"summary\":\"x\"}"}}]}}]}));
        let events = agg.finish(None);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], StreamEvent::ToolCalls(v) if v[0].name == "remember"));
        assert_eq!(
            events[1],
            StreamEvent::Done {
                finish_reason: None
            }
        );
    }

    #[test]
    fn usage_chunk_is_forwarded_even_with_empty_choices() {
        // include_usage 的尾 chunk 形如 {"choices":[], "usage":{...}}：
        // usage 检查必须在 choices[0] 守卫之前，否则整个 chunk 被丢弃。
        let mut agg = ToolStreamAggregator::new();
        let usage =
            json!({"prompt_tokens": 1200, "prompt_tokens_details": {"cached_tokens": 1024}});
        let events = agg.feed_chunk(&json!({"choices":[], "usage": usage.clone()}));
        assert_eq!(events, vec![StreamEvent::Usage(usage)]);
    }

    #[test]
    fn usage_tail_chunk_after_finish_reason_is_forwarded() {
        // 真实协议顺序回归：tool_calls 分片 → 带 finish_reason 的 chunk →
        // {"choices":[],"usage":{...}} 尾 chunk → [DONE]。
        // usage 必须照常透传，且事件序严格为 ToolCalls → Usage → Done。
        let mut agg = ToolStreamAggregator::new();
        assert_eq!(agg.feed_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"roll_check","arguments":"{\"tested_parameter\":\"DEX\"}"}}]}}]})), Vec::<StreamEvent>::new());
        // finish_reason chunk：分片已齐 → flush ToolCalls；Done 暂存待 [DONE]。
        assert_eq!(
            agg.feed_chunk(&json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})),
            vec![StreamEvent::ToolCalls(vec![AggregatedToolCall {
                id: "call_1".to_string(),
                name: "roll_check".to_string(),
                arguments: "{\"tested_parameter\":\"DEX\"}".to_string()
            }]),]
        );
        // usage 尾 chunk 在 finish_reason 之后到达，不能被 done 守卫吞掉。
        let usage =
            json!({"prompt_tokens": 1200, "prompt_tokens_details": {"cached_tokens": 1024}});
        assert_eq!(
            agg.feed_chunk(&json!({"choices":[], "usage": usage.clone()})),
            vec![StreamEvent::Usage(usage)]
        );
        // [DONE]：发 Done，finish_reason 用暂存的协议值；ToolCalls 不重复发。
        assert_eq!(
            agg.finish(None),
            vec![StreamEvent::Done {
                finish_reason: Some("tool_calls".to_string())
            }]
        );
    }
}
