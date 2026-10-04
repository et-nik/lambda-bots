use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// The settings cannot work: a bad URL, no root certificates for HTTPS.
    #[error("{0}")]
    Config(String),
    #[error("HTTP {status}: {message}")]
    Status {
        status: u16,
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
                status, retry_after, ..
            } => match status {
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
        LlmError::Status {
            status,
            message,
            retry_after: retry_after.and_then(|v| parse_retry_after(v, SystemTime::now())),
        }
    }
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
    use super::*;

    fn status(code: u16) -> LlmError {
        LlmError::Status {
            status: code,
            message: String::new(),
            retry_after: Some(Duration::from_secs(7)),
        }
    }

    #[test]
    fn classes() {
        for code in [401, 402, 403, 404] {
            assert_eq!(status(code).class(), ErrorClass::Fatal, "{code}");
        }
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
        let openai = r#"{"error":{"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"}}"#;
        let e = LlmError::status(429, openai, Some("3"));
        assert_eq!(e.to_string(), "HTTP 429: Rate limit reached");
        assert_eq!(e.class(), ErrorClass::Backoff(Some(Duration::from_secs(3))));
        let e = LlmError::status(502, "<html>Bad Gateway</html>", None);
        assert_eq!(e.to_string(), "HTTP 502: <html>Bad Gateway</html>");
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
