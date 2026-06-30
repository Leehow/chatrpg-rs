use crate::model::EvalFixture;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalParseError {
    #[error("missing eval-fixture JSON code block")]
    MissingFixtureBlock,
    #[error("invalid eval-fixture JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

pub fn parse_markdown_fixture(input: &str) -> Result<EvalFixture, EvalParseError> {
    let json = extract_eval_fixture_block(input).ok_or(EvalParseError::MissingFixtureBlock)?;
    Ok(serde_json::from_str(json.trim())?)
}

fn extract_eval_fixture_block(input: &str) -> Option<String> {
    let mut in_block = false;
    let mut out = String::new();
    for line in input.lines() {
        let trimmed = line.trim_start();
        if !in_block {
            if trimmed.starts_with("```") && trimmed.contains("eval-fixture") {
                in_block = true;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            return Some(out);
        }
        out.push_str(line);
        out.push('\n');
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_eval_fixture_json_block() {
        let text = r#"
# x
```json eval-fixture
{"fixture_id":"f","title":"t","turns":[]}
```
"#;
        let fixture = parse_markdown_fixture(text).unwrap();
        assert_eq!(fixture.fixture_id, "f");
    }

    #[test]
    fn missing_block_is_error() {
        let err = parse_markdown_fixture("# no block").unwrap_err();
        assert!(matches!(err, EvalParseError::MissingFixtureBlock));
    }
}
