//! The HTTP agent: rustls with ring, the system's root certificates plus an optional PEM bundle, bounded in time.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig, TlsProvider};

use crate::LlmError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Chat completions are short; anything this long is not one.
const MAX_BODY: u64 = 1 << 20;

pub(crate) struct Response {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: String,
}

/// `https`: the requests go over TLS, so some root certificate must be known.
pub(crate) fn agent(timeout: Duration, ca_file: Option<&Path>, https: bool) -> Result<ureq::Agent, LlmError> {
    let roots = roots(ca_file)?;
    if https && roots.is_empty() {
        return Err(LlmError::Config(
            "no root certificates in the system: set ca_file (or SSL_CERT_FILE) to a PEM bundle".into(),
        ));
    }
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::new_with_certs(&roots))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    Ok(ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(timeout))
        .timeout_connect(Some(CONNECT_TIMEOUT.min(timeout)))
        .user_agent(concat!("lambdabots/", env!("CARGO_PKG_VERSION")))
        .tls_config(tls)
        .build()
        .new_agent())
}

fn roots(ca_file: Option<&Path>) -> Result<Vec<Certificate<'static>>, LlmError> {
    let mut roots: Vec<Certificate<'static>> = rustls_native_certs::load_native_certs()
        .certs
        .iter()
        .map(|der| Certificate::from_der(der.as_ref()).to_owned())
        .collect();
    if let Some(path) = ca_file {
        let pem = std::fs::read(path).map_err(|e| LlmError::Config(format!("ca_file {}: {e}", path.display())))?;
        let before = roots.len();
        for item in ureq::tls::parse_pem(&pem) {
            if let Ok(PemItem::Certificate(cert)) = item {
                roots.push(cert);
            }
        }
        if roots.len() == before {
            return Err(LlmError::Config(format!("ca_file {}: no certificates", path.display())));
        }
    }
    Ok(roots)
}

pub(crate) fn post_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<Response, LlmError> {
    let mut request = agent.post(url).header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let mut response = request.send(body).map_err(transport)?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(transport)?;
    Ok(Response {
        status,
        retry_after,
        body,
    })
}

fn transport(e: ureq::Error) -> LlmError {
    match e {
        ureq::Error::BadUri(uri) => LlmError::Config(format!("bad URL: {uri}")),
        e => LlmError::Transport(e.to_string()),
    }
}
