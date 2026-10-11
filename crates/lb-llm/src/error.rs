use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// The settings cannot work: a bad URL, no root certificates for HTTPS.
    #[error("{0}")]
    Config(String),
    #[error("HTTP {status}: {message}")]
    Status {
        status: u16,
        /// The provider's name for the error: `error.type`, else `error.code` when it is a string
        /// (`rate_limit_error`, `insufficient_quota`); empty when the answer has neither.
        kind: String,
        message: String,
        retry_after: Option<Duration>,
    },
    /// The server was not reached or did not answer in time.
    #[error("{0}")]
    Transport(String),
    /// The server answered with something that is not a chat completion.
    #[error("unexpected response: {0}")]
    Body(String),
}

/// What the caller should do after an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    /// Asking again will not help (bad key, unknown model, no access): stop until the settings change.
    Fatal,
    /// The account has no money or quota left: asking again helps only after a top-up.
    Billing,
    /// The service is busy or out of reach: wait, at least as long as it asked when it did.
    Backoff(Option<Duration>),
    /// This request was wrong; the next one may be fine.
    Drop,
}

impl LlmError {
    pub fn class(&self) -> ErrorClass {
        match self {
            LlmError::Config(_) => ErrorClass::Fatal,
            LlmError::Status {
                status,
                kind,
                message,
                retry_after,
            } => match status {
                402 => ErrorClass::Billing,
                400 | 403 | 429 if unpaid(kind, message) => ErrorClass::Billing,
                401..=404 => ErrorClass::Fatal,
                408 | 409 | 429 | 500..=599 => ErrorClass::Backoff(*retry_after),
                _ => ErrorClass::Drop,
            },
            LlmError::Transport(_) => ErrorClass::Backoff(None),
            LlmError::Body(_) => ErrorClass::Drop,
        }
    }

    /// An error answer: the message both APIs put in `error.message`, else the start of the body.
    pub(crate) fn status(status: u16, body: &str, retry_after: Option<&str>) -> LlmError {
        let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
        let message = parsed
            .as_ref()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.get("message"))
                    .or_else(|| v.get("detail"))
                    .and_then(|m| m.as_str())
            })
            .map(str::to_string)
            .unwrap_or_else(|| body.chars().take(200).collect::<String>().trim().to_string());
        let kind = parsed
            .as_ref()
            .and_then(|v| {
                ["/error/type", "/error/code"]
                    .iter()
                    .find_map(|path| v.pointer(path).and_then(|k| k.as_str()))
            })
            .unwrap_or_default()
            .to_string();
        LlmError::Status {
            status,
            kind,
            message,
            retry_after: retry_after.and_then(|v| parse_retry_after(v, SystemTime::now())),
        }
    }
}

/// Words that mark a 400, 403 or 429 as an account without money or quota, looked for in its kind and message in
/// lower case. A bare `billing` and "exceeded your current quota" are left out: plain rate limits say them too
/// (OpenAI's free tier and Groq point at a billing page, Gemini's 429 says that phrase). So is `suspended`: a key
/// blocked for other reasons stays [`ErrorClass::Fatal`].
const UNPAID: &[&str] = &[
    "insufficient_quota",
    "exceeded_current_quota",
    "billing_error",
    "credit balance",
    "insufficient balance",
    "balance is insufficient",
    "insufficient credit",
    "recharge",
    "api usage limits",
];

fn unpaid(kind: &str, message: &str) -> bool {
    let text = format!("{kind} {message}").to_lowercase();
    UNPAID.iter().any(|w| text.contains(w))
}

/// `Retry-After`: delay seconds, or an HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) as a delay from `now`.
pub(crate) fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs.min(86_400.0)));
    }
    let at = http_date(value)?;
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(at.saturating_sub(now)))
}

/// Seconds since the epoch of an IMF-fixdate.
fn http_date(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts.as_slice() else {
        return None;
    };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.iter().position(|m| m == month)? as i64 + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let mut hms = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, sec) = (hms.next()??, hms.next()??, hms.next()??);
    if !(1..=31).contains(&day) || !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..=60).contains(&sec) {
        return None;
    }
    // Days from 1970-01-01 to the civil date (H. Hinnant's `days_from_civil`).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + m * 60 + sec).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn status(code: u16) -> LlmError {
        LlmError::Status {
            status: code,
            kind: String::new(),
            message: String::new(),
            retry_after: Some(Duration::from_secs(7)),
        }
    }

    fn kind(e: &LlmError) -> &str {
        match e {
            LlmError::Status { kind, .. } => kind,
            _ => "",
        }
    }

    #[test]
    fn classes() {
        for code in [401, 403, 404] {
            assert_eq!(status(code).class(), ErrorClass::Fatal, "{code}");
        }
        assert_eq!(status(402).class(), ErrorClass::Billing);
        for code in [408, 429, 500, 503, 529] {
            assert_eq!(
                status(code).class(),
                ErrorClass::Backoff(Some(Duration::from_secs(7))),
                "{code}"
            );
        }
        for code in [400, 413, 422] {
            assert_eq!(status(code).class(), ErrorClass::Drop, "{code}");
        }
        let said = |code, message: &str| LlmError::Status {
            status: code,
            kind: String::new(),
            message: message.into(),
            retry_after: None,
        };
        assert_eq!(said(403, "Insufficient balance").class(), ErrorClass::Billing);
        assert_eq!(said(403, "This API key is suspended").class(), ErrorClass::Fatal);
        assert_eq!(
            said(422, "the prompt says: insufficient balance, recharge").class(),
            ErrorClass::Drop,
            "a 422 may quote the prompt"
        );
        assert_eq!(
            LlmError::Transport("timed out".into()).class(),
            ErrorClass::Backoff(None)
        );
        assert_eq!(LlmError::Config("no roots".into()).class(), ErrorClass::Fatal);
        assert_eq!(LlmError::Body("no choices".into()).class(), ErrorClass::Drop);
    }

    #[test]
    fn messages_from_both_apis() {
        let anthropic = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let e = LlmError::status(401, anthropic, None);
        assert_eq!(e.to_string(), "HTTP 401: invalid x-api-key");
        assert_eq!(kind(&e), "authentication_error");
        let openai = r#"{"error":{"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"}}"#;
        let e = LlmError::status(429, openai, Some("3"));
        assert_eq!(e.to_string(), "HTTP 429: Rate limit reached");
        assert_eq!(kind(&e), "requests");
        assert_eq!(e.class(), ErrorClass::Backoff(Some(Duration::from_secs(3))));
        let e = LlmError::status(502, "<html>Bad Gateway</html>", None);
        assert_eq!(e.to_string(), "HTTP 502: <html>Bad Gateway</html>");
        assert_eq!(kind(&e), "");
        let coded = r#"{"error":{"message":"no money","type":null,"code":"insufficient_quota"}}"#;
        assert_eq!(kind(&LlmError::status(429, coded, None)), "insufficient_quota");
    }

    #[test]
    fn unpaid_accounts() {
        let billing = |status, body: serde_json::Value| {
            let e = LlmError::status(status, &body.to_string(), None);
            assert_eq!(e.class(), ErrorClass::Billing, "{e}");
            kind(&e).to_string()
        };
        // Moonshot's refusals once the money ran out, the account and key ids replaced: the first six, then the rest.
        for message in [
            "Your credit balance is running low for account <org-test>, please recharge immediately to avoid service \
             disruption",
            "Your account org-test <ak-test> is suspended due to insufficient balance, please recharge your account or \
             check your plan and billing details",
        ] {
            let body = json!({"error": {"message": message, "type": "exceeded_current_quota_error"}});
            assert_eq!(billing(429, body), "exceeded_current_quota_error");
        }
        // The rest are the providers' documented texts (their docs and public bug reports), not seen in our logs.
        // OpenAI tells it by the kind alone.
        let openai = json!({"error": {
            "message": "You exceeded your current quota, please check your plan and billing details. For more \
                        information on this error, read the docs: \
                        https://platform.openai.com/docs/guides/error-codes/api-errors.",
            "type": "insufficient_quota", "param": null, "code": "insufficient_quota"
        }});
        assert_eq!(billing(429, openai), "insufficient_quota");
        // Anthropic: no credit left, the usage tier's monthly spend cap, a spend limit the organization set.
        let no_credit = json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to \
                        upgrade or purchase credits."
        }});
        assert_eq!(billing(400, no_credit), "invalid_request_error");
        let tier_cap = json!({"type": "error", "error": {
            "type": "rate_limit_error",
            "message": "You have reached your API usage limits: your organization has crossed its monthly API usage \
                        threshold, set based on your organization's API tier. You will regain access on 2026-09-01 at \
                        00:00 UTC.",
            "details": {"error_code": "enforced_spend_limit_reached"}
        }});
        assert_eq!(billing(429, tier_cap), "rate_limit_error");
        let own_cap = json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": "You have reached your specified API usage limits. You will regain access on 2026-09-01 at \
                        00:00 UTC."
        }});
        assert_eq!(billing(400, own_cap), "invalid_request_error");
        // DeepSeek and OpenRouter answer 402; OpenRouter's code is a number, which names no kind.
        let deepseek = json!({"error": {
            "message": "Insufficient Balance", "type": "unknown_error", "param": null, "code": "invalid_request_error"
        }});
        assert_eq!(billing(402, deepseek), "unknown_error");
        let openrouter = json!({"error": {
            "code": 402, "message": "Insufficient credits. Add more using https://openrouter.ai/credits"
        }});
        assert_eq!(billing(402, openrouter), "");
    }

    #[test]
    fn busy_is_not_unpaid() {
        // Rate limits in the providers' documented texts: they speak of billing and quota, yet a wait clears them.
        let openai = json!({"error": {
            "message": "Rate limit reached for gpt-4o-mini in organization org-test on requests per min (RPM): \
                        Limit 3, Used 3, Requested 1. Please try again in 20s. Visit \
                        https://platform.openai.com/account/rate-limits to learn more. You can increase your rate \
                        limit by adding a payment method to your account at \
                        https://platform.openai.com/account/billing.",
            "type": "requests", "param": null, "code": "rate_limit_exceeded"
        }});
        let groq = json!({"error": {
            "message": "Rate limit reached for model `llama-3.3-70b-versatile` in organization `org_test` service \
                        tier `on_demand` on tokens per minute (TPM): Limit 12000, Used 11000, Requested 1500. Please \
                        try again in 2.5s. Need more tokens? Upgrade to Dev Tier today at \
                        https://console.groq.com/settings/billing",
            "type": "tokens", "code": "rate_limit_exceeded"
        }});
        let gemini = json!({"error": {
            "code": 429,
            "message": "You exceeded your current quota, please check your plan and billing details. For more \
                        information on this error, head to: https://ai.google.dev/gemini-api/docs/rate-limits.",
            "status": "RESOURCE_EXHAUSTED"
        }});
        for body in [openai, groq, gemini] {
            let e = LlmError::status(429, &body.to_string(), None);
            assert_eq!(e.class(), ErrorClass::Backoff(None), "{e}");
        }
    }

    #[test]
    fn retry_after_forms() {
        let now = UNIX_EPOCH + Duration::from_secs(784_111_777); // Sun, 06 Nov 1994 08:49:37 GMT
        assert_eq!(parse_retry_after("120", now), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after(" 1.5 ", now), Some(Duration::from_millis(1500)));
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:50:07 GMT", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(
            parse_retry_after("Thu, 29 Feb 2024 00:00:00 GMT", UNIX_EPOCH),
            Some(Duration::from_secs(1_709_164_800))
        );
        assert_eq!(parse_retry_after("soon", now), None);
        assert_eq!(parse_retry_after("-3", now), None);
    }
}
