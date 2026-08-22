pub mod llm;
pub mod stt;

use crate::store::{Result, Store};

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
