use serde_json::Value;

/// 一条从 OpenAI 兼容 SSE chat 流里解出来的数据项。
#[derive(Debug, Clone, PartialEq)]
pub enum SseItem {
    /// 成功解析的 `data:` chunk JSON。
    Data(Value),
    /// `data: [DONE]` 结束哨兵。
    Done,
}

/// OpenAI 兼容 SSE chat 字节流的增量解码器，与 reqwest 解耦（单测直接喂字节）。
///
/// 持有一个增长的 UTF-8 buffer，用 `String::drain` **就地**消费已完整的
/// `\n` 结尾行——而不是旧实现的 `buffer = buffer[pos+1..].to_string()`
/// 每行重新分配/复制整条剩余 buffer（大流时 O(n²)）。
/// `data:` chunk 的 JSON 解析失败会被计数（`parse_errors`）并 `tracing::warn!`
/// 出来，而不是 `Err(_) => continue` 静默吞掉。
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
    parse_errors: u64,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 至今为止 JSON 解析失败的 `data:` chunk 数量（每次失败也会 `tracing::warn!`）。
    pub fn parse_errors(&self) -> u64 {
        self.parse_errors
    }

    /// 尚未消费的尾部（还没等到 `\n` 的半行）。供测试断言"就地增量消费"。
    pub fn buffered(&self) -> &str {
        &self.buffer
    }

    /// 追加一段原始字节（lossy UTF-8）到 buffer。
    pub fn push_bytes(&mut self, chunk: &[u8]) {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
    }

    /// 排干当前 buffer 里所有已完整的行，把每个 `data:` 行解成 `SseItem`，按流序返回。
    /// 非 `data:` 前缀行（注释/空行）跳过；尾部半行（无 `\n`）留在 buffer 等下次。
    /// 遇到 `[DONE]` 时把 `SseItem::Done` 作为最后一项返回并**停止排干**（保留其后字节，
    /// 与原实现遇 `[DONE]` 即 `break` 的语义一致）。
    pub fn drain_items(&mut self) -> Vec<SseItem> {
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim().to_string();
            // 就地消费这一行（含换行符），不重新分配剩余 buffer。
            self.buffer.drain(..pos + 1);
            if !line.starts_with("data:") {
                continue;
            }
            let data = line.trim_start_matches("data:").trim();
            if data == "[DONE]" {
                out.push(SseItem::Done);
                return out;
            }
            match serde_json::from_str::<Value>(data) {
                Ok(value) => out.push(SseItem::Data(value)),
                Err(err) => {
                    self.parse_errors += 1;
                    tracing::warn!(
                        target: "trpg_llm::sse",
                        llm_stream_parse_errors = self.parse_errors,
                        error = %err,
                        snippet = %data.chars().take(120).collect::<String>(),
                        "discarded unparseable SSE data chunk",
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 从一批 SseItem 里抽出 `choices[0].delta.content` 文本增量（按序）。
    fn content_deltas(items: &[SseItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|it| match it {
                SseItem::Data(v) => v
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                SseItem::Done => None,
            })
            .collect()
    }

    #[test]
    fn drains_data_lines_in_order_consuming_incrementally() {
        let mut dec = SseDecoder::new();
        // 第一个 chunk：两条完整行 + 第三条半行（无结尾 \n）。
        dec.push_bytes(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\
              data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\
              data: {\"choices\":[{\"delta\":{\"content\":\", wor",
        );
        let first = dec.drain_items();
        assert_eq!(content_deltas(&first), vec!["He", "llo"]);
        // 半行原样留在 buffer——证明就地增量消费，没有把尾巴整条复制丢掉。
        assert_eq!(
            dec.buffered(),
            "data: {\"choices\":[{\"delta\":{\"content\":\", wor"
        );
        // 第二个 chunk 补全半行并追加 [DONE]。
        dec.push_bytes(b"ld\"}}]}\ndata: [DONE]\n");
        let second = dec.drain_items();
        assert_eq!(content_deltas(&second), vec![", world"]);
        assert!(matches!(second.last(), Some(SseItem::Done)));
        assert_eq!(dec.parse_errors(), 0);
    }

    #[test]
    fn malformed_json_line_is_counted_not_silently_dropped() {
        let mut dec = SseDecoder::new();
        dec.push_bytes(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"ok1\"}}]}\n\
              data: {not valid json]\n\
              data: {\"choices\":[{\"delta\":{\"content\":\"ok2\"}}]}\n",
        );
        let items = dec.drain_items();
        // 合法行照常按序解析出来。
        assert_eq!(content_deltas(&items), vec!["ok1", "ok2"]);
        // 坏行被计数，而不是静默吞掉（修复前这里会是 0）。
        assert_eq!(dec.parse_errors(), 1);
    }

    #[test]
    fn done_sentinel_stops_draining_and_leaves_remainder() {
        let mut dec = SseDecoder::new();
        dec.push_bytes(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\
              data: [DONE]\n\
              data: {\"choices\":[{\"delta\":{\"content\":\"after\"}}]}\n",
        );
        let items = dec.drain_items();
        assert_eq!(content_deltas(&items), vec!["hi"]);
        assert!(matches!(items.last(), Some(SseItem::Done)));
        // [DONE] 之后的行在同一次排干里**不**被处理（对齐原实现的 break）；
        // 其字节仍留在 buffer（调用方靠停止消费来丢弃）。
        assert!(dec.buffered().contains("after"));
    }

    #[test]
    fn legacy_text_field_is_decoded() {
        // 兼容 completion 风格的 choices[0].text（lib.rs/stream_tools 都读它）。
        let mut dec = SseDecoder::new();
        dec.push_bytes(b"data: {\"choices\":[{\"text\":\"raw\"}]}\n");
        let items = dec.drain_items();
        assert_eq!(items.len(), 1);
        let text = match &items[0] {
            SseItem::Data(v) => v
                .pointer("/choices/0/text")
                .and_then(Value::as_str)
                .map(str::to_string),
            SseItem::Done => None,
        };
        assert_eq!(text.as_deref(), Some("raw"));
    }
}
