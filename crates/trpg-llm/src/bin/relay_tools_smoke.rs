// crates/trpg-llm/src/bin/relay_tools_smoke.rs
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use trpg_llm::{LlmClient, LlmConfig, OpenAiCompatibleClient};

fn echo_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "echo_probe",
            "description": "Return the exact probe text.",
            "parameters": {
                "type": "object",
                "properties": {
                    "probe": {"type": "string"}
                },
                "required": ["probe"],
                "additionalProperties": false
            }
        }
    })
}

async fn smoke_complete_with_tools_multi_turn(client: &OpenAiCompatibleClient) -> Result<()> {
    let tool = echo_tool();
    let mut messages = vec![
        json!({"role":"system","content":"You are a relay smoke test. Call echo_probe exactly once, then answer normally after the tool result."}),
        json!({"role":"user","content":"Call echo_probe with probe='relay-multi-turn-ok'."}),
    ];
    let first = client.complete_with_tools(messages.clone(), vec![tool.clone()]).await?;
    let msg = first.pointer("/choices/0/message").cloned().ok_or_else(|| anyhow!("first response missing message"))?;
    let calls = msg.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
    if calls.is_empty() {
        return Err(anyhow!("complete_with_tools did not return tool_calls in first turn"));
    }
    messages.push(msg);
    for call in calls {
        let id = call.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("tool call missing id"))?;
        messages.push(json!({
            "role": "tool",
            "tool_call_id": id,
            "name": "echo_probe",
            "content": "{\"ok\":true,\"echo\":\"relay-multi-turn-ok\"}"
        }));
    }
    let second = client.complete_with_tools(messages, vec![tool]).await?;
    let content = second.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Err(anyhow!("second tool-history turn produced no content"));
    }
    println!("PASS complete_with_tools multi-turn: {}", content.trim().chars().take(120).collect::<String>());
    Ok(())
}

async fn collect_raw_sse(config: &LlmConfig, body: Value) -> Result<(bool, bool, bool)> {
    let http = Client::new();
    let mut stream = http
        .post(config.endpoint("chat/completions"))
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()
        .await
        .context("raw SSE request failed")?
        .error_for_status()
        .context("raw SSE response status was not success")?
        .bytes_stream();
    let mut buffer = String::new();
    let mut saw_tool_delta = false;
    let mut saw_content_delta = false;
    let mut saw_usage = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();
            if !line.starts_with("data:") {
                continue;
            }
            let data = line.trim_start_matches("data:").trim();
            println!("SSE {data}");
            if data == "[DONE]" {
                continue;
            }
            let value: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if value.pointer("/choices/0/delta/tool_calls").is_some() {
                saw_tool_delta = true;
            }
            if value.pointer("/choices/0/delta/content").and_then(Value::as_str).map(|s| !s.is_empty()).unwrap_or(false) {
                saw_content_delta = true;
            }
            if value.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
                saw_usage = true;
                println!("USAGE cached_tokens={:?}", value.pointer("/usage/prompt_tokens_details/cached_tokens"));
            }
        }
    }
    Ok((saw_tool_delta, saw_content_delta, saw_usage))
}

async fn smoke_raw_stream_tool_delta(config: &LlmConfig) -> Result<()> {
    let tool_body = json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [
            {"role":"system","content":"Call echo_probe once. Use a long probe argument so the relay is likely to split function arguments across SSE chunks."},
            {"role":"user","content":"Call echo_probe with probe='alpha-bravo-charlie-delta-echo-foxtrot-golf-hotel-india-juliet-kilo'."}
        ],
        "tools": [echo_tool()],
        // 实测偏差：OpenAI 嵌套命名形式 {"type":"function","function":{"name":..}} 被
        // relay 上游 400 拒（"Missing required parameter: 'tool_choice.name'"，Responses
        // API 扁平命名形式）。生产 ToolChoice 只有 Auto/None，命名强制仅 smoke 用，
        // 故此处用上游接受的扁平形式。
        "tool_choice": {"type":"function","name":"echo_probe"}
    });
    let (tool_delta, _, tool_usage) = collect_raw_sse(config, tool_body).await?;
    if !tool_delta {
        return Err(anyhow!("raw stream did not expose tool_calls delta"));
    }

    let content_body = json!({
        "model": config.model,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [
            {"role":"system","content":"Do not call tools. Reply with the exact sentence: relay content delta ok."},
            {"role":"user","content":"Return the sentence now."}
        ],
        "tools": [echo_tool()],
        "tool_choice": "none"
    });
    let (_, content_delta, content_usage) = collect_raw_sse(config, content_body).await?;
    if !content_delta {
        return Err(anyhow!("raw stream did not expose content delta"));
    }
    // usage 透传仅观测记录（spec §6.1 第 5 条），不作 go/no-go 闸门。
    println!("INFO usage passthrough: tool_round={tool_usage} content_round={content_usage}");
    println!("PASS raw SSE stream exposes tool_calls delta and content delta");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = LlmConfig::from_env()?;
    eprintln!("relay smoke target: base_url={} model={}", config.base_url, config.model);
    let client = OpenAiCompatibleClient::new(config.clone())?;
    smoke_complete_with_tools_multi_turn(&client).await?;
    smoke_raw_stream_tool_delta(&config).await?;
    println!("PASS relay_tools_smoke all checks");
    Ok(())
}
