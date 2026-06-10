pub mod stream_tools;
pub use stream_tools::*;

use anyhow::{anyhow, Context, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::pin::Pin;
use std::time::Duration;
use trpg_model::ChatMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_secs: u64,
    pub send_temperature: bool,
}

impl LlmConfig {
    pub fn openai(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: api_key.into(),
            model: model.into(),
            timeout_secs: 180,
            send_temperature: true,
        }
    }

    pub fn from_env() -> Result<Self> {
        let provider = std::env::var("TRPG_LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());
        let base_url = std::env::var("TRPG_LLM_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let api_key = std::env::var("TRPG_LLM_API_KEY").context("TRPG_LLM_API_KEY is not set")?;
        let model = std::env::var("TRPG_LLM_MODEL").unwrap_or_else(|_| "gpt-4.1".to_string());
        let timeout_secs = std::env::var("TRPG_LLM_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(180);
        let send_temperature = std::env::var("TRPG_LLM_SEND_TEMPERATURE")
            .ok()
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or_else(|| !base_url.to_ascii_lowercase().contains("codex-relay"));
        Ok(Self { provider, base_url, api_key, model, timeout_secs, send_temperature })
    }

    pub fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete_text(&self, messages: Vec<ChatMessage>, temperature: f32) -> Result<String>;
    async fn complete_json(&self, messages: Vec<ChatMessage>, temperature: f32) -> Result<Value>;
    async fn stream_chat(&self, messages: Vec<ChatMessage>, temperature: f32) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;
    /// OpenAI function-calling turn. `messages` raw JSON (may carry `tool_calls`
    /// / a `tool` role), `tools` are function schemas. Returns the full response
    /// Value (`/choices/0/message`, `/usage`). Default: unsupported. The
    /// rulebook-reader agent requires this (text pseudo-tool protocols make gpt
    /// refuse).
    async fn complete_with_tools(&self, _messages: Vec<Value>, _tools: Vec<Value>) -> Result<Value> {
        Err(anyhow!("complete_with_tools is not supported by this LlmClient"))
    }

    /// 流式 function-calling 回合（D2 硬依赖）。`messages`/`tools` 为 OpenAI raw
    /// JSON（与 complete_with_tools 同构：可携带 assistant tool_calls / tool role
    /// 历史）。默认 unsupported（与 complete_with_tools 同模式）。
    async fn stream_chat_with_tools(
        &self,
        _messages: Vec<Value>,
        _tools: Vec<Value>,
        _tool_choice: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Err(anyhow!("stream_chat_with_tools is not supported by this LlmClient"))
    }
}

#[derive(Clone)]
pub struct OpenAiCompatibleClient {
    config: LlmConfig,
    http: Client,
}

impl OpenAiCompatibleClient {
    pub fn new(config: LlmConfig) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;
        Ok(Self { config, http })
    }

    fn messages_json(messages: Vec<ChatMessage>) -> Vec<Value> {
        messages.into_iter().map(|m| json!({"role": m.role, "content": m.content})).collect()
    }

    fn base_body(&self, messages: Vec<ChatMessage>, stream: bool, temperature: f32) -> Value {
        let mut body = json!({
            "model": self.config.model.clone(),
            "messages": Self::messages_json(messages),
            "stream": stream
        });
        if self.config.send_temperature {
            body["temperature"] = json!(temperature);
        }
        body
    }

    fn max_retries(&self) -> usize {
        std::env::var("TRPG_LLM_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
    }

    fn retry_base_delay_ms(&self) -> u64 {
        std::env::var("TRPG_LLM_RETRY_BASE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(800)
    }

    fn is_retryable_status(status: StatusCode) -> bool {
        status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::BAD_GATEWAY
            || status == StatusCode::SERVICE_UNAVAILABLE
            || status == StatusCode::GATEWAY_TIMEOUT
            || status.is_server_error()
    }

    fn retry_after_delay_ms(resp: &reqwest::Response, fallback_ms: u64) -> u64 {
        resp.headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|secs| secs.saturating_mul(1000))
            .unwrap_or(fallback_ms)
    }

    async fn post_chat(&self, body: Value) -> Result<Value> {
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..=self.max_retries() {
            let resp = self.http
                .post(self.config.endpoint("chat/completions"))
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await;
            match resp {
                Ok(resp) => {
                    let status = resp.status();
                    let retry_delay_ms = Self::retry_after_delay_ms(&resp, self.retry_base_delay_ms().saturating_mul(1u64 << attempt.min(5)));
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        return serde_json::from_str(&text).with_context(|| format!("invalid LLM JSON response: {}", text.chars().take(500).collect::<String>()));
                    }
                    let err = anyhow!("LLM API error {status}: {text}");
                    if attempt < self.max_retries() && Self::is_retryable_status(status) {
                        tracing::warn!(attempt = attempt + 1, status = %status, retry_delay_ms, "retrying non-streaming LLM request");
                        tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(err) => {
                    let retry_delay_ms = self.retry_base_delay_ms().saturating_mul(1u64 << attempt.min(5));
                    let err = anyhow!(err).context("LLM transport error");
                    if attempt < self.max_retries() {
                        tracing::warn!(attempt = attempt + 1, retry_delay_ms, error = %err, "retrying non-streaming LLM transport error");
                        tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("LLM request failed without a recorded error")))
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatibleClient {
    async fn complete_with_tools(&self, messages: Vec<Value>, tools: Vec<Value>) -> Result<Value> {
        let body = json!({
            "model": self.config.model.clone(),
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "stream": false,
        });
        self.post_chat(body).await
    }

    async fn stream_chat_with_tools(
        &self,
        messages: Vec<Value>,
        tools: Vec<Value>,
        tool_choice: ToolChoice,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.stream_chat_with_tools_impl(messages, tools, tool_choice).await
    }

    async fn complete_text(&self, messages: Vec<ChatMessage>, temperature: f32) -> Result<String> {
        let body = self.base_body(messages, false, temperature);
        let value = self.post_chat(body).await?;
        let content = value.pointer("/choices/0/message/content").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("LLM response missing choices[0].message.content"))?;
        Ok(content.to_string())
    }

    async fn complete_json(&self, messages: Vec<ChatMessage>, temperature: f32) -> Result<Value> {
        let mut body = self.base_body(messages, false, temperature);
        body["response_format"] = json!({"type": "json_object"});
        let value = self.post_chat(body).await?;
        let content = value.pointer("/choices/0/message/content").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("LLM response missing choices[0].message.content"))?;
        parse_json_from_llm(content)
    }

    async fn stream_chat(&self, messages: Vec<ChatMessage>, temperature: f32) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let body = self.base_body(messages, true, temperature);
        let mut last_error: Option<anyhow::Error> = None;
        let mut resp_opt = None;
        for attempt in 0..=self.max_retries() {
            let resp = self.http
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
                    let retry_delay_ms = Self::retry_after_delay_ms(&resp, self.retry_base_delay_ms().saturating_mul(1u64 << attempt.min(5)));
                    let text = resp.text().await.unwrap_or_default();
                    let err = anyhow!("LLM API error {status}: {text}");
                    if attempt < self.max_retries() && Self::is_retryable_status(status) {
                        tracing::warn!(attempt = attempt + 1, status = %status, retry_delay_ms, "retrying streaming LLM request before first token");
                        tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(err) => {
                    let retry_delay_ms = self.retry_base_delay_ms().saturating_mul(1u64 << attempt.min(5));
                    let err = anyhow!(err).context("LLM streaming transport error before first token");
                    if attempt < self.max_retries() {
                        tracing::warn!(attempt = attempt + 1, retry_delay_ms, error = %err, "retrying streaming LLM transport error before first token");
                        tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        let resp = resp_opt.ok_or_else(|| last_error.unwrap_or_else(|| anyhow!("LLM streaming request failed without a recorded error")))?;
        let mut bytes = resp.bytes_stream();
        let s = try_stream! {
            let mut buffer = String::new();
            let mut done = false;
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer = buffer[pos + 1..].to_string();
                    if !line.starts_with("data:") {
                        continue;
                    }
                    let data = line.trim_start_matches("data:").trim();
                    if data == "[DONE]" {
                        done = true;
                        break;
                    }
                    let parsed: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(delta) = parsed.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                        if !delta.is_empty() {
                            yield delta.to_string();
                        }
                    }
                    if let Some(text) = parsed.pointer("/choices/0/text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            yield text.to_string();
                        }
                    }
                }
                if done {
                    break;
                }
            }
        };
        Ok(Box::pin(s))
    }
}

pub fn system(content: impl Into<String>) -> ChatMessage {
    ChatMessage { role: "system".to_string(), content: content.into() }
}

pub fn user(content: impl Into<String>) -> ChatMessage {
    ChatMessage { role: "user".to_string(), content: content.into() }
}

pub fn assistant(content: impl Into<String>) -> ChatMessage {
    ChatMessage { role: "assistant".to_string(), content: content.into() }
}

pub fn parse_json_from_llm(content: &str) -> Result<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(content) {
        return Ok(v);
    }
    let trimmed = content.trim();
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            let slice = &trimmed[start..=end];
            return serde_json::from_str(slice).with_context(|| "failed to parse JSON object extracted from LLM response");
        }
    }
    Err(anyhow!("LLM response was not JSON: {}", content.chars().take(500).collect::<String>()))
}

pub struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete_text(&self, _messages: Vec<ChatMessage>, _temperature: f32) -> Result<String> {
        Ok("Mock LLM response.".to_string())
    }
    async fn complete_json(&self, _messages: Vec<ChatMessage>, _temperature: f32) -> Result<Value> {
        Ok(json!({"mock": true}))
    }
    async fn stream_chat(&self, _messages: Vec<ChatMessage>, _temperature: f32) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let s = try_stream! {
            yield "Mock ".to_string();
            yield "streamed ".to_string();
            yield "response.".to_string();
        };
        Ok(Box::pin(s))
    }
}
