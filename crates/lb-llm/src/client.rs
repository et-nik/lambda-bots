use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::{LlmError, anthropic, http, openai};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `POST {base}/v1/messages`.
    Anthropic,
    /// `POST {base}/chat/completions`, `base` usually ending in `/v1`.
    OpenAi,
}

#[derive(Clone)]
pub struct Settings {
    pub kind: Kind,
    /// Empty: the provider's own address (OpenAI-compatible servers have none).
    pub base_url: String,
    pub model: String,
    pub key: Option<String>,
    /// Sent with every request (a gateway's own header, say).
    pub headers: Vec<(String, String)>,
    pub max_tokens: u32,
    /// Sent only when set: some models refuse sampling parameters.
    pub temperature: Option<f32>,
    /// A JSON object merged into every request body; `null` values remove fields.
    pub extra_body: Option<serde_json::Value>,
    pub timeout: Duration,
    /// PEM certificates trusted on top of the system's.
    pub ca_file: Option<PathBuf>,
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("key", &self.key.as_ref().map(|_| "<set>"))
            .field("headers", &self.headers.iter().map(|(k, _)| k).collect::<Vec<_>>())
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("extra_body", &self.extra_body)
            .field("timeout", &self.timeout)
            .field("ca_file", &self.ca_file)
            .finish()
    }
}

/// One request: instructions and the message to answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    /// Instructions shared by many requests; Anthropic caches them where they are long enough.
    pub system_static: String,
    /// Instructions for this request only.
    pub system: String,
    pub user: String,
    /// Overrides [`Settings::max_tokens`].
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The model finished its answer.
    End,
    /// Cut at `max_tokens`.
    MaxTokens,
    /// The model or a safety filter declined to answer.
    Refusal,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub text: String,
    pub stop: Stop,
    /// Input tokens, cached ones included.
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub struct Client {
    settings: Settings,
    url: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(settings: Settings) -> Result<Client, LlmError> {
        let base = match (settings.kind, settings.base_url.trim()) {
            (Kind::Anthropic, "") => anthropic::DEFAULT_URL.to_string(),
            (Kind::OpenAi, "") => return Err(LlmError::Config("an OpenAI-compatible provider needs base_url".into())),
            (_, base) => base.trim_end_matches('/').to_string(),
        };
        if !base.starts_with("http://") && !base.starts_with("https://") {
            return Err(LlmError::Config(format!("base_url `{base}` is not an http(s) URL")));
        }
        let secret = settings
            .key
            .as_ref()
            .map(|_| "the API key".to_string())
            .or_else(|| {
                let (name, _) = settings.headers.iter().find(|(name, _)| credential(name))?;
                Some(format!("the {name} header"))
            })
            .or_else(|| userinfo(&base).then(|| "the user and password in it".to_string()));
        if let Some(secret) = secret
            && base.starts_with("http://")
            && !loopback(&base)
        {
            return Err(LlmError::Config(format!(
                "base_url `{base}` would send {secret} unencrypted: use https:// or a loopback address"
            )));
        }
        let url = match settings.kind {
            Kind::Anthropic => anthropic::url(&base),
            Kind::OpenAi => openai::url(&base),
        };
        let agent = http::agent(
            settings.timeout,
            settings.ca_file.as_deref(),
            url.starts_with("https://"),
        )?;
        Ok(Client { settings, url, agent })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The address requests go to.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn complete(&self, prompt: &Prompt) -> Result<Completion, LlmError> {
        let s = &self.settings;
        let (mut body, mut headers) = match s.kind {
            Kind::Anthropic => (anthropic::body(s, prompt), anthropic::headers(s.key.as_deref())),
            Kind::OpenAi => (openai::body(s, prompt), openai::headers(s.key.as_deref())),
        };
        if let Some(extra) = &s.extra_body {
            merge(&mut body, extra);
        }
        if s.kind == Kind::OpenAi && body.get("max_completion_tokens").is_some() {
            body.as_object_mut().map(|o| o.remove("max_tokens"));
        }
        headers.extend(s.headers.iter().cloned());
        let response = http::post_json(&self.agent, &self.url, &headers, &body.to_string())?;
        if !(200..300).contains(&response.status) {
            return Err(LlmError::status(
                response.status,
                &response.body,
                response.retry_after.as_deref(),
            ));
        }
        match s.kind {
            Kind::Anthropic => anthropic::parse(&response.body),
            Kind::OpenAi => openai::parse(&response.body),
        }
    }
}

/// `localhost` or a loopback address, the host read as the agent reads it: plain HTTP to it stays on this machine.
fn loopback(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    let host = uri.host().unwrap_or_default();
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `http://user:password@host`: the agent sends the user and the password as basic authorization.
fn userinfo(url: &str) -> bool {
    url.parse::<ureq::http::Uri>()
        .is_ok_and(|uri| uri.authority().is_some_and(|a| a.as_str().contains('@')))
}

/// A header that carries a credential, judged by its name: `authorization`, `x-api-key`, `x-gateway-token` and the like.
fn credential(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["auth", "key", "token", "secret", "password", "credential", "cookie"]
        .iter()
        .any(|w| name.contains(w))
}

/// Merges `extra` into `body` object by object; a `null` removes the field.
fn merge(body: &mut serde_json::Value, extra: &serde_json::Value) {
    let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (key, value) in extra {
        match (body.get_mut(key), value) {
            (_, serde_json::Value::Null) => {
                body.remove(key);
            }
            (Some(existing), serde_json::Value::Object(_)) if existing.is_object() => merge(existing, value),
            _ => {
                body.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{Canned, MockServer};

    fn settings(kind: Kind, base: &str) -> Settings {
        Settings {
            kind,
            base_url: base.into(),
            model: "test-model".into(),
            key: Some("sk-test".into()),
            headers: vec![("x-gateway".into(), "lb".into())],
            max_tokens: 100,
            temperature: None,
            extra_body: None,
            timeout: Duration::from_secs(5),
            ca_file: None,
        }
    }

    fn prompt() -> Prompt {
        Prompt {
            system_static: "rules".into(),
            system: "you are Bob".into(),
            user: "say hi".into(),
            max_tokens: None,
        }
    }

    #[test]
    fn merge_adds_replaces_and_removes() {
        let mut body = json!({"model": "a", "max_tokens": 5, "output_config": {"x": 1}});
        merge(
            &mut body,
            &json!({"max_tokens": null, "output_config": {"effort": "low"}, "top_k": 3}),
        );
        assert_eq!(
            body,
            json!({"model": "a", "output_config": {"x": 1, "effort": "low"}, "top_k": 3})
        );
    }

    #[test]
    fn anthropic_round_trip() {
        let server = MockServer::start(vec![Canned::json(
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"test-model",
                "content":[{"type":"text","text":"привет всем"}],"stop_reason":"end_turn",
                "usage":{"input_tokens":120,"cache_read_input_tokens":30,"output_tokens":7}}"#,
        )]);
        let client = Client::new(settings(Kind::Anthropic, &server.url())).unwrap();
        let done = client.complete(&prompt()).unwrap();
        assert_eq!(done.text, "привет всем");
        assert_eq!(done.stop, Stop::End);
        assert_eq!((done.input_tokens, done.output_tokens), (150, 7));
        let req = &server.requests()[0];
        assert_eq!(req.path, "/v1/messages");
        assert_eq!(req.header("x-api-key"), Some("sk-test"));
        assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(req.header("x-gateway"), Some("lb"));
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["system"][0]["text"], "rules");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["system"][1]["text"], "you are Bob");
        assert_eq!(body["messages"][0], json!({"role": "user", "content": "say hi"}));
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn openai_round_trip_with_extra_body() {
        let server = MockServer::start(vec![Canned::json(
            200,
            r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"<think>hmm</think>gg wp"},
                "finish_reason":"stop"}],"usage":{"prompt_tokens":90,"completion_tokens":4}}"#,
        )]);
        let mut s = settings(Kind::OpenAi, &format!("{}/v1/", server.url()));
        s.temperature = Some(0.9);
        s.extra_body = Some(json!({"max_completion_tokens": 64}));
        let client = Client::new(s).unwrap();
        let done = client.complete(&prompt()).unwrap();
        assert_eq!(done.text, "gg wp");
        assert_eq!((done.input_tokens, done.output_tokens), (90, 4));
        let req = &server.requests()[0];
        assert_eq!(req.path, "/v1/chat/completions");
        assert_eq!(req.header("authorization"), Some("Bearer sk-test"));
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "rules\n\nyou are Bob");
        assert_eq!(body["messages"][1]["content"], "say hi");
        assert_eq!(body["max_completion_tokens"], 64);
        assert!(body.get("max_tokens").is_none());
        assert!((body["temperature"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn chunked_answers_and_errors() {
        let ok = r#"{"content":[{"type":"text","text":"lol"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let server = MockServer::start(vec![
            Canned::json(200, ok).chunked(),
            Canned::json(
                429,
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
            )
            .header("retry-after", "2"),
            Canned::json(
                401,
                r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
            ),
        ]);
        let client = Client::new(settings(Kind::Anthropic, &server.url())).unwrap();
        assert_eq!(client.complete(&prompt()).unwrap().text, "lol");
        let e = client.complete(&prompt()).unwrap_err();
        assert_eq!(e.class(), crate::ErrorClass::Backoff(Some(Duration::from_secs(2))));
        let e = client.complete(&prompt()).unwrap_err();
        assert_eq!(e.class(), crate::ErrorClass::Fatal);
        assert!(e.to_string().contains("bad key"));
    }

    #[test]
    fn timeouts_are_transport_errors() {
        let server = MockServer::start(vec![Canned::json(200, "{}").delay(Duration::from_millis(1500))]);
        let mut s = settings(Kind::Anthropic, &server.url());
        s.timeout = Duration::from_millis(300);
        let client = Client::new(s).unwrap();
        let started = std::time::Instant::now();
        let e = client.complete(&prompt()).unwrap_err();
        assert!(matches!(e, LlmError::Transport(_)), "{e:?}");
        assert!(started.elapsed() < Duration::from_millis(1200));
    }

    #[test]
    fn bad_settings() {
        assert!(matches!(
            Client::new(settings(Kind::OpenAi, "")),
            Err(LlmError::Config(_))
        ));
        assert!(matches!(
            Client::new(settings(Kind::Anthropic, "api.example.com")),
            Err(LlmError::Config(_))
        ));
        let s = settings(Kind::Anthropic, "");
        assert!(!format!("{s:?}").contains("sk-test"));
    }

    #[test]
    fn keys_go_over_plain_http_only_to_loopback() {
        let keyless = |base: &str, header: Option<&str>| {
            let mut s = settings(Kind::OpenAi, base);
            s.key = None;
            s.headers
                .extend(header.map(|name| (name.to_string(), "g-456".to_string())));
            Client::new(s)
        };
        for base in [
            "http://127.0.0.1:8099/v1",
            "http://localhost:11434/v1",
            "http://[::1]:8080/v1",
            "https://gw.example.com/v1",
            "http://lb:secret@localhost:8099/v1",
        ] {
            assert!(Client::new(settings(Kind::OpenAi, base)).is_ok(), "{base}");
            assert!(keyless(base, Some("X-Gateway-Key")).is_ok(), "{base}");
        }
        for base in [
            "http://192.168.8.10:8080/v1",
            "http://gw.example.com/v1",
            "http://127.0.0.1@gw.example.com/v1",
            "http://lb:secret@gw.example.com/v1",
        ] {
            assert!(
                matches!(Client::new(settings(Kind::OpenAi, base)), Err(LlmError::Config(_))),
                "{base}"
            );
            for name in ["Authorization", "x-api-key", "X-Gateway-Key", "x-gateway-token"] {
                assert!(
                    matches!(keyless(base, Some(name)), Err(LlmError::Config(_))),
                    "{base}: {name}"
                );
            }
            assert_eq!(
                keyless(base, None).is_ok(),
                !base.contains('@'),
                "{base}: x-gateway carries no key, a user in the URL does"
            );
        }
    }
}
