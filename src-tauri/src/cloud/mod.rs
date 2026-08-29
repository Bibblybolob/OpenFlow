pub mod llm;
pub mod local_llm;
#[cfg(feature = "parakeet")]
pub mod local_parakeet;
pub mod local_stt;
pub mod stt;

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;

use crate::store::{Result, Store, StoreError};

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

/// Process-wide client for model downloads. A total request timeout would
/// reject large offline models after two minutes even when bytes continue to
/// arrive. The read timeout only fires when a transfer stalls.
pub fn http_client() -> Result<Client> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .tcp_nodelay(true)
        .build()
        .map_err(|e| StoreError::Other(e.to_string()))?;
    let _ = HTTP_CLIENT.set(client);
    Ok(HTTP_CLIENT.get().expect("client was just inserted").clone())
}

/// Streams a response with a connection timeout and a stalled-read timeout.
/// There is no total transfer timeout, so a healthy long download can finish.
pub fn stream_download<F>(url: &str, mut on_chunk: F) -> Result<(Option<u64>, u64)>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let client = http_client()?;
    stream_download_with_client(client, url, &mut on_chunk)
}

fn stream_download_with_client<F>(
    client: Client,
    url: &str,
    on_chunk: &mut F,
) -> Result<(Option<u64>, u64)>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| StoreError::Other(format!("download runtime failed: {e}")))?;

    runtime.block_on(async move {
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|e| StoreError::Other(format!("model download failed: {e}")))?;
        if !response.status().is_success() {
            return Err(StoreError::Other(format!(
                "model download failed ({})",
                response.status()
            )));
        }

        let expected_size = response.content_length();
        let mut downloaded = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| StoreError::Other(format!("model download failed: {e}")))?
        {
            downloaded += chunk.len() as u64;
            on_chunk(&chunk)?;
        }
        Ok((expected_size, downloaded))
    })
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
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

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

    #[test]
    fn stalled_download_returns_after_the_read_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\na")
                .unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .read_timeout(Duration::from_millis(20))
            .build()
            .unwrap();

        let result =
            stream_download_with_client(client, &format!("http://{address}"), &mut |_chunk| Ok(()));
        server.join().unwrap();
        assert!(result.is_err(), "a stalled response must time out");
    }
}
