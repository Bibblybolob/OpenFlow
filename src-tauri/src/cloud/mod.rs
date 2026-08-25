pub mod llm;
pub mod local_llm;
pub mod local_stt;
pub mod stt;

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::blocking::Client;

use crate::store::{Result, Store, StoreError};

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

/// Process-wide blocking client. The connection pool keeps TLS sessions warm
/// between dictations, so each STT/cleanup request reuses a live connection
/// instead of paying a fresh DNS+TCP+TLS handshake (~100-300ms per call).
/// Per-request timeouts override the client default where needed.
pub fn http_client() -> Result<Client> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .build()
        .map_err(|e| StoreError::Other(e.to_string()))?;
    let _ = HTTP_CLIENT.set(client);
    Ok(HTTP_CLIENT.get().expect("client was just inserted").clone())
}

/// Local fast-path: if the whole utterance matches a snippet trigger,
/// return its body without calling any API.
pub fn try_snippet(db: &Store, raw_text: &str) -> Result<Option<String>> {
    let normalized = normalize(raw_text);
    if normalized.is_empty() {
        return Ok(None);
    }
    for s in db.list_snippets()? {
        if normalize(&s.trigger) == normalized {
            return Ok(Some(s.body));
        }
    }
    Ok(None)
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn expands_exact_trigger_case_insensitive() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        store.add_snippet("my email", "jon@example.com").unwrap();
        assert_eq!(
            try_snippet(&store, "  MY   EMAIL ").unwrap(),
            Some("jon@example.com".to_string())
        );
    }

    #[test]
    fn no_match_returns_none() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        store.add_snippet("my email", "jon@example.com").unwrap();
        assert_eq!(try_snippet(&store, "send my email please").unwrap(), None);
        assert_eq!(try_snippet(&store, "").unwrap(), None);
    }
}
