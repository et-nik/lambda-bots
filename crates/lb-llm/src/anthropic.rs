//! The Anthropic Messages API (`anthropic-version: 2023-06-01`).

use serde::Deserialize;
use serde_json::json;

use crate::{Completion, LlmError, Prompt, Settings, Stop};

pub(crate) const DEFAULT_URL: &str = "https://api.anthropic.com";
const VERSION: &str = "2023-06-01";

pub(crate) fn url(base: &str) -> String {
    format!("{base}/v1/messages")
}

pub(crate) fn headers(key: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![("anthropic-version".to_string(), VERSION.to_string())];
    if let Some(key) = key {
        headers.push(("x-api-key".to_string(), key.to_string()));
    }
    headers
}

pub(crate) fn body(s: &Settings, p: &Prompt) -> serde_json::Value {
    let mut system = Vec::new();
    if !p.system_static.is_empty() {
        system.push(json!({"type": "text", "text": p.system_static, "cache_control": {"type": "ephemeral"}}));
    }
    if !p.system.is_empty() {
        system.push(json!({"type": "text", "text": p.system}));
    }
    let mut body = json!({
        "model": s.model,
        "max_tokens": p.max_tokens.unwrap_or(s.max_tokens),
        "messages": [{"role": "user", "content": p.user}],
    });
    if !system.is_empty() {
        body["system"] = system.into();
    }
    if let Some(t) = s.temperature {
        body["temperature"] = json!(t);
    }
    body
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Vec<Block>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

pub(crate) fn parse(body: &str) -> Result<Completion, LlmError> {
    let m: Message = serde_json::from_str(body).map_err(|e| LlmError::Body(e.to_string()))?;
    let text: String = m
        .content
        .iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text.as_str())
        .collect();
    let stop = match m.stop_reason.as_deref() {
        Some("end_turn" | "stop_sequence") | None => Stop::End,
        Some("max_tokens") => Stop::MaxTokens,
        Some("refusal") => Stop::Refusal,
        Some(other) => Stop::Other(other.to_string()),
    };
    let (input_tokens, output_tokens) = m.usage.map_or((0, 0), |u| {
        (
            u.input_tokens + u.cache_creation_input_tokens.unwrap_or(0) + u.cache_read_input_tokens.unwrap_or(0),
            u.output_tokens,
        )
    });
    Ok(Completion {
        text,
        stop,
        input_tokens,
        output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_text_blocks_and_thinking() {
        let body = r#"{"content":[{"type":"thinking","thinking":"","signature":"x"},
            {"type":"text","text":"ну "},{"type":"text","text":"и ладно"}],
            "stop_reason":"max_tokens","usage":{"input_tokens":10,"output_tokens":100}}"#;
        let c = parse(body).unwrap();
        assert_eq!(c.text, "ну и ладно");
        assert_eq!(c.stop, Stop::MaxTokens);
    }

    #[test]
    fn refusal_and_empty_content() {
        let c = parse(r#"{"content":[],"stop_reason":"refusal","stop_details":{"type":"refusal","category":null}}"#)
            .unwrap();
        assert_eq!((c.text.as_str(), c.stop), ("", Stop::Refusal));
        assert!(matches!(parse("not json"), Err(LlmError::Body(_))));
    }
}
