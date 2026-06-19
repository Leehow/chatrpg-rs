/// 终轮 content delta 经此过滤后才下发玩家（唯一在线护栏，spec §7）。
/// 原理：holdback 尾窗 = max(token 字节长)-1 暂扣；尾窗+新 delta 拼接后做
/// 大小写不敏感扫描，命中私骰 token 替换为 "■"；tokens 为空 ⇒ 零暂扣完全透明直通。
pub struct RedactingBuffer {
    tokens: Vec<String>,
    tail: String,
    holdback: usize,
}

impl RedactingBuffer {
    pub fn new(private_tokens: Vec<String>) -> Self {
        let mut tokens = private_tokens
            .into_iter()
            .map(|s| s.to_ascii_lowercase())
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>();
        tokens.sort();
        tokens.dedup();
        let holdback = tokens
            .iter()
            .map(|s| s.len().saturating_sub(1))
            .max()
            .unwrap_or(0);
        Self {
            tokens,
            tail: String::new(),
            holdback,
        }
    }

    /// 喂入一个 delta，返回此刻可安全下发的文本（可能为空串：尾窗暂扣中）。
    pub fn push(&mut self, delta: &str) -> String {
        if self.tokens.is_empty() {
            return delta.to_string();
        }
        let mut joined = String::new();
        joined.push_str(&self.tail);
        joined.push_str(delta);
        // 防漏顺序（契约第 6 节）：必须先对 joined 整体 redact、再切分尾窗。
        // 若先切分再只 redact safe 前缀，结尾落在尾窗内的完整 token 会被切成
        // 两半——前半随 safe 明文下发、后半留在 tail 永远凑不齐完整 token，
        // 两段都不命中扫描 → 整个 token 泄漏（如 delta 恰以 token 结尾）。
        // redact 后残留的"部分 token 后缀"至多 max(token 长)-1 = holdback 字节，
        // 必然整体落入 tail，下次 push/finish 拼接后补扫，不漏。
        let redacted = self.redact(&joined);
        // holdback 是字节数，切分点可能落在多字节 UTF-8 字符（中文叙事/■）中间：
        // 必须回退到最近的 char boundary，否则字节切 String 直接 panic。
        let mut split_at = redacted.len().saturating_sub(self.holdback);
        while split_at > 0 && !redacted.is_char_boundary(split_at) {
            split_at -= 1;
        }
        let safe = redacted[..split_at].to_string();
        self.tail = redacted[split_at..].to_string();
        safe
    }

    /// 流结束：排空尾窗，返回最后一段安全文本。
    pub fn finish(&mut self) -> String {
        if self.tokens.is_empty() {
            return String::new();
        }
        let tail = std::mem::take(&mut self.tail);
        self.redact(&tail)
    }

    fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for token in &self.tokens {
            let numeric = !token.is_empty() && token.chars().all(|c| c.is_ascii_digit());
            let mut from = 0usize;
            loop {
                // to_ascii_lowercase 仅改写 ASCII，字节布局与 out 完全一致，
                // pos/end 可直接用于 out 的 replace_range。
                let lower = out.to_ascii_lowercase();
                let Some(rel) = lower[from..].find(token.as_str()) else {
                    break;
                };
                let pos = from + rel;
                let end = pos + token.len();
                if numeric && !numeric_boundary(&out, pos, end) {
                    from = pos + 1; // 跳过本误命中点继续扫（token 首字节是 ASCII 数字，pos+1 必为 char boundary）
                    continue;
                }
                if numeric && numeric_public_target_context(&out, pos) {
                    from = pos + 1;
                    continue;
                }
                out.replace_range(pos..end, "■");
                from = pos + "■".len();
            }
        }
        out
    }
}

/// 数字 token 词边界（终审 important）：前后均非 ASCII 字母数字才命中——
/// "1934" 里的 "34"、"roll345" 里的 "34" 不涂；CJK 紧邻数字（"掷出34点"）
/// 仍命中（CJK 不延伸数字字面量，漏放才是泄密方向）。
fn numeric_boundary(text: &str, pos: usize, end: usize) -> bool {
    let before_ok = text[..pos]
        .chars()
        .next_back()
        .map_or(true, |c| !c.is_ascii_alphanumeric());
    let after_ok = text[end..]
        .chars()
        .next()
        .map_or(true, |c| !c.is_ascii_alphanumeric());
    before_ok && after_ok
}

fn numeric_public_target_context(text: &str, pos: usize) -> bool {
    let before = text[..pos]
        .chars()
        .rev()
        .take(18)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let before = before.to_ascii_lowercase();
    ["目标", "target", "tn", "dv", "dc", "difficulty", "难度"]
        .iter()
        .any(|cue| before.contains(cue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_private_token_across_chunk_boundary() {
        let mut b = RedactingBuffer::new(vec!["91".to_string()]);
        let a = b.push("The hidden total is 9");
        let c = b.push("1, but you only see movement.");
        let d = b.finish();
        let out = format!("{a}{c}{d}");
        assert!(!out.contains("91"));
        assert!(out.contains("■"));
    }

    #[test]
    fn redacts_token_ending_at_delta_boundary() {
        // 泄漏场景①+②回归：delta 恰好以完整 token 结尾（LLM 流式最常见形态）。
        // 先切分再 redact 的旧实现会把 token 切成两半（前半明文下发、后半留
        // tail 永远凑不齐），两段都扫不中 → 泄漏。含长 token（1d100，holdback>1）
        // 结尾落在 delta 末 holdback 字节内的变体。
        let mut b = RedactingBuffer::new(vec!["91".to_string(), "1d100".to_string()]);
        let a = b.push("The hidden total is 91");
        let c = b.push(", and the GM rolled 1d100");
        let d = b.push(" in secret.");
        let e = b.finish();
        let out = format!("{a}{c}{d}{e}");
        assert!(!out.contains("91"), "token at delta boundary leaked: {out}");
        assert!(
            !out.contains("1d100"),
            "long token at delta boundary leaked: {out}"
        );
        assert!(out.contains("■"));
        assert!(out.contains(" in secret."));
    }

    #[test]
    fn redacts_token_at_end_of_stream() {
        // 泄漏场景③回归：流以完整 token 结尾（push 一次后直接 finish）。
        let mut b = RedactingBuffer::new(vec!["91".to_string()]);
        let a = b.push("The hidden total is 91");
        let d = b.finish();
        let out = format!("{a}{d}");
        assert!(!out.contains("91"), "token at end of stream leaked: {out}");
        assert!(out.contains("■"));
    }

    #[test]
    fn narrative_words_and_embedded_numbers_survive_narrowed_tokens() {
        // 终审 important 回归：收窄后的过滤集只剩 roll_id+点数；数字 token 词边界
        // 匹配——"successfully"（"success" 已不在集合）与年份 "1934"（"34" 前邻
        // 数字）不被涂黑；裸点数 "34" 单独出现仍被涂黑。
        let mut b = RedactingBuffer::new(vec!["roll_b".to_string(), "34".to_string()]);
        let a = b.push("He successfully recalled the year 1934, ");
        let c = b.push("then the hidden die showed 34 exactly.");
        let d = b.finish();
        let out = format!("{a}{c}{d}");
        assert_eq!(
            out,
            "He successfully recalled the year 1934, then the hidden die showed ■ exactly."
        );
    }

    #[test]
    fn numeric_token_adjacent_to_cjk_is_still_redacted() {
        // 泄密方向 fail-closed：CJK 紧邻数字不构成更长的数字字面量，"掷出34点"
        // 中的真实点数必须涂黑（词边界只认 ASCII 字母数字为延伸）。
        let mut b = RedactingBuffer::new(vec!["34".to_string()]);
        let a = b.push("你掷出34点，骰子停了。");
        let d = b.finish();
        let out = format!("{a}{d}");
        assert!(
            !out.contains("34"),
            "CJK-adjacent point value leaked: {out}"
        );
        assert!(out.contains("■"));
    }

    #[test]
    fn numeric_token_does_not_redact_public_roll_target_context() {
        let mut b = RedactingBuffer::new(vec!["40".to_string()]);
        let a = b.push("[roll]Stealth：1d100 = 90，目标 40，失败。[/roll]");
        let d = b.finish();
        let out = format!("{a}{d}");
        assert!(
            out.contains("目标 40"),
            "public target values must remain visible: {out}"
        );
        assert!(!out.contains("目标 ■"));
    }

    #[test]
    fn empty_tokens_are_transparent() {
        let mut b = RedactingBuffer::new(vec![]);
        assert_eq!(b.push("abc"), "abc");
        assert_eq!(b.finish(), "");
    }

    #[test]
    fn utf8_chinese_with_token_across_chunk_boundary_does_not_panic() {
        // 中文叙事是 e2e 主场景（血色公路）：holdback 字节切分点落在多字节
        // 字符中间时必须回退到 char boundary，否则 byte-index panic。
        let mut b = RedactingBuffer::new(vec!["秘密91".to_string()]);
        let a = b.push("你看到了秘");
        let c = b.push("密91的暗示，但什么都没有发生。");
        let d = b.finish();
        let out = format!("{a}{c}{d}");
        assert!(!out.contains("秘密91"));
        assert!(out.contains("■"));
        assert!(out.contains("但什么都没有发生"));
    }
}
