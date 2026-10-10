//! OpenAI-compatible chat completions (OpenAI, OpenRouter, vLLM, Ollama, llama.cpp and the like).

use serde::Deserialize;
use serde_json::json;

use crate::{Completion, LlmError, Prompt, Settings, Stop};

pub(crate) fn url(base: &str) -> String {
    format!("{base}/chat/completions")
}

pub(crate) fn headers(key: Option<&str>) -> Vec<(String, String)> {
    key.filter(|k| !k.is_empty())
        .map(|k| vec![("authorization".to_string(), format!("Bearer {k}"))])
        .unwrap_or_default()
}

pub(crate) fn body(s: &Settings, p: &Prompt) -> serde_json::Value {
    let system = [p.system_static.as_str(), p.system.as_str()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": p.user}));
    let mut body = json!({
        "model": s.model,
        "max_tokens": p.max_tokens.unwrap_or(s.max_tokens),
        "messages": messages,
    });
    if let Some(t) = s.temperature {
        body["temperature"] = json!(t);
    }
    body
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<Message>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

pub(crate) fn parse(body: &str) -> Result<Completion, LlmError> {
    let r: Response = serde_json::from_str(body).map_err(|e| LlmError::Body(e.to_string()))?;
    let choice = r
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::Body("no choices".into()))?;
    let refused = choice.message.as_ref().is_some_and(|m| m.refusal.is_some());
    let text = choice
        .message
        .and_then(|m| m.content)
        .map(|t| without_thinking(&t))
        .unwrap_or_default();
    let stop = match choice.finish_reason.as_deref() {
        _ if refused => Stop::Refusal,
        Some("stop") | None => Stop::End,
        Some("length") => Stop::MaxTokens,
        Some("content_filter") => Stop::Refusal,
        Some(other) => Stop::Other(other.to_string()),
    };
    let (input_tokens, output_tokens) = r.usage.map_or((0, 0), |u| (u.prompt_tokens, u.completion_tokens));
    Ok(Completion {
        text,
        stop,
        input_tokens,
        output_tokens,
    })
}

/// Reasoning models served this way put their thinking in `<think>…</think>` before the answer; an answer cut
/// inside its thinking has none.
fn without_thinking(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out.trim().to_string(),
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_is_dropped() {
        assert_eq!(without_thinking("<think>a\nb</think>\nok"), "ok");
        assert_eq!(without_thinking("<think>never ends"), "");
        assert_eq!(without_thinking("plain"), "plain");
    }

    #[test]
    fn null_content_refusal_and_no_choices() {
        let c =
            parse(r#"{"choices":[{"message":{"role":"assistant","content":null},"finish_reason":"length"}]}"#).unwrap();
        assert_eq!((c.text.as_str(), c.stop), ("", Stop::MaxTokens));
        let c = parse(r#"{"choices":[{"message":{"content":null,"refusal":"no"},"finish_reason":"stop"}]}"#).unwrap();
        assert_eq!(c.stop, Stop::Refusal);
        assert!(matches!(parse(r#"{"choices":[]}"#), Err(LlmError::Body(_))));
    }
}
