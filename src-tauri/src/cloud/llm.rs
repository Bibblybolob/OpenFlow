use serde_json::{json, Value};

use crate::store::{Result, Store, StoreError};

const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-3-5-haiku-latest";
const DEFAULT_OPENROUTER_MODEL: &str = "anthropic/claude-3.5-haiku";
const MAX_PROMPT_CHARS: usize = 6_000;

pub const SYSTEM_PROMPT: &str = "You clean up raw speech-to-text dictation. Rewrite the dictated text as polished written prose.\n\
Rules:\n\
- Remove filler words (\"um\", \"uh\", \"you know\").\n- When the speaker changes their mind mid-sentence — \"wait\", \"actually\", \"never mind\", \"or rather\", or simply restarting a sentence — keep only the final intent and drop every abandoned word. Example: \"send it to John wait no to Jane tomorrow\" becomes \"Send it to Jane tomorrow.\"\n\
- Add correct punctuation, capitalization, and paragraph breaks.\n\
- Honor spoken formatting commands: \"new line\", \"new paragraph\", \"bullet list\", \"numbered list\".\n\
- Preserve the input language, meaning, tone, and any code-like tokens or file paths.\n\
- Use the preferred vocabulary exactly as given for names and terms.\n- Honor spoken emoji requests: \"insert party emoji\" appends 🎉 at that spot; without such a request, never add emojis.\n\
- Never answer the content, never add information, never comment. Output ONLY the cleaned text.";

pub fn polish(
    db: &Store,
    raw_text: &str,
    app_identifier: &str,
    context: Option<&str>,
) -> Result<String> {
    let cfg = resolve_config(db)?;
    let style = resolve_style_instructions(db, app_identifier)?.unwrap_or_default();
    let body = build_request_body(cfg.provider, &cfg.model, raw_text, &style, context, db)?;

    let req = match cfg.provider {
        Provider::OpenAi => super::http_client()?
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&cfg.api_key),
        Provider::Anthropic => super::http_client()?
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", "2023-06-01"),
        Provider::OpenRouter => super::http_client()?
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(&cfg.api_key),
    };

    let resp = req
        .timeout(std::time::Duration::from_secs(15))
        .json(&body)
        .send()
        .map_err(|e| StoreError::Other(format!("cleanup request failed: {e}")))?;

    let status = resp.status();
    let text = resp.text().map_err(|e| StoreError::Other(e.to_string()))?;
    if !status.is_success() {
        return Err(StoreError::Other(format!(
            "cleanup failed ({status}): {}",
            truncate(&text, 300)
        )));
    }

    extract_text(cfg.provider, &text)
}

/// Tone instructions for a session: an explicit pill override (styleOverride
/// setting) wins over automatic per-app matching.
fn resolve_style_instructions(db: &Store, app_identifier: &str) -> Result<Option<String>> {
    if let Some(raw) = db.get_setting("styleOverride").ok().flatten() {
        if let Ok(id) = serde_json::from_str::<i64>(&raw) {
            return db.style_instructions_by_id(id);
        }
        // Explicit "no style" sentinel chosen in the pill: suppress even
        // per-app matching instead of falling back to it.
        if raw.trim() == "\"none\"" {
            return Ok(None);
        }
    }
    db.resolve_style_for_app(app_identifier)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Anthropic,
    OpenRouter,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
            Provider::OpenRouter => "openrouter",
        }
    }
}

pub struct CleanupConfig {
    pub provider: Provider,
    pub api_key: String,
    pub model: String,
}

impl std::fmt::Debug for CleanupConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupConfig")
            .field("provider", &self.provider.as_str())
            .field("api_key", &"***")
            .field("model", &self.model)
            .finish()
    }
}

fn setting_string(db: &Store, key: &str) -> Option<String> {
    db.get_setting(key)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
}

/// Resolution order per provider key: Settings value first, then env var.
/// Provider selection: explicit `llmProvider` setting wins; otherwise auto —
/// whichever key is available (OpenAI → Anthropic → OpenRouter). Errors name
/// all options.
pub fn resolve_config(db: &Store) -> Result<CleanupConfig> {
    let key_from = |setting_key: &str, env_key: &str| -> Option<String> {
        setting_string(db, setting_key)
            .filter(|k| !k.trim().is_empty())
            .or_else(|| std::env::var(env_key).ok().filter(|k| !k.trim().is_empty()))
    };
    let openai_key = key_from("openaiApiKey", "OPENAI_API_KEY");
    let anthropic_key = key_from("anthropicApiKey", "ANTHROPIC_API_KEY");
    let openrouter_key = key_from("openrouterApiKey", "OPENROUTER_API_KEY");

    let provider = match setting_string(db, "llmProvider").as_deref() {
        Some("openai") => Some(Provider::OpenAi),
        Some("anthropic") => Some(Provider::Anthropic),
        Some("openrouter") => Some(Provider::OpenRouter),
        _ => None,
    }
    .or(match (&openai_key, &anthropic_key, &openrouter_key) {
        (Some(_), _, _) => Some(Provider::OpenAi),
        (None, Some(_), _) => Some(Provider::Anthropic),
        (None, None, Some(_)) => Some(Provider::OpenRouter),
        (None, None, None) => None,
    });

    let model_setting = setting_string(db, "llmModel").filter(|m| !m.trim().is_empty());

    match provider {
        Some(Provider::OpenAi) => Ok(CleanupConfig {
            api_key: openai_key.ok_or(missing_key_err("openai"))?,
            model: model_setting.unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string()),
            provider: Provider::OpenAi,
        }),
        Some(Provider::Anthropic) => Ok(CleanupConfig {
            api_key: anthropic_key.ok_or(missing_key_err("anthropic"))?,
            model: model_setting.unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string()),
            provider: Provider::Anthropic,
        }),
        Some(Provider::OpenRouter) => Ok(CleanupConfig {
            api_key: openrouter_key.ok_or(missing_key_err("openrouter"))?,
            model: model_setting.unwrap_or_else(|| DEFAULT_OPENROUTER_MODEL.to_string()),
            provider: Provider::OpenRouter,
        }),
        None => Err(StoreError::Other(
            "no LLM configured — add an OpenAI, Claude, or OpenRouter API key in Settings"
                .to_string(),
        )),
    }
}

fn missing_key_err(provider: &str) -> StoreError {
    StoreError::Other(format!(
        "{provider} cleanup selected but its API key is missing — set it in Settings"
    ))
}

pub fn build_request_body(
    provider: Provider,
    model: &str,
    raw_text: &str,
    style_instructions: &str,
    context: Option<&str>,
    db: &Store,
) -> Result<Value> {
    let user_prompt = build_user_prompt(raw_text, style_instructions, context, db)?;
    Ok(match provider {
        // OpenRouter speaks the OpenAI chat-completions dialect.
        Provider::OpenAi | Provider::OpenRouter => json!({
            "model": model,
            "temperature": 0,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user_prompt}
            ]
        }),
        Provider::Anthropic => json!({
            "model": model,
            "max_tokens": 2048,
            "temperature": 0,
            "system": SYSTEM_PROMPT,
            "messages": [
                {"role": "user", "content": user_prompt}
            ]
        }),
    })
}

fn build_user_prompt(
    raw_text: &str,
    style_instructions: &str,
    context: Option<&str>,
    db: &Store,
) -> Result<String> {
    let mut prompt = String::new();

    if let Some(ctx) = context.map(str::trim).filter(|c| !c.is_empty()) {
        prompt.push_str(&format!(
            "Text already before the cursor (continue it coherently — do not repeat or answer it): \n\"{}\"\n\n",
            truncate(ctx, 400)
        ));
    }

    if !style_instructions.trim().is_empty() {
        prompt.push_str(&format!(
            "Style instructions: {}\n",
            style_instructions.trim()
        ));
    }

    let terms: Vec<String> = db
        .list_dictionary()?
        .into_iter()
        .take(50)
        .map(|e| e.replacement.clone().unwrap_or(e.term))
        .collect();
    if !terms.is_empty() {
        prompt.push_str(&format!("Preferred vocabulary: {}\n", terms.join(", ")));
    }

    let snippets: Vec<String> = db
        .list_snippets()?
        .into_iter()
        .take(30)
        .map(|s| format!("“{}” → {}", s.trigger, truncate(&s.body, 200)))
        .collect();
    if !snippets.is_empty() {
        prompt.push_str(&format!(
            "If the dictation contains one of these spoken cues, expand it to the matching text: {}\n",
            snippets.join("; ")
        ));
    }

    if !prompt.is_empty() {
        prompt.push('\n');
    }
    prompt.push_str("Dictation:\n");
    let remaining = MAX_PROMPT_CHARS.saturating_sub(prompt.len());
    prompt.push_str(&truncate(raw_text, remaining));

    Ok(prompt)
}

pub fn extract_text(provider: Provider, response_body: &str) -> Result<String> {
    let parsed: Value = serde_json::from_str(response_body)
        .map_err(|e| StoreError::Other(format!("bad cleanup response: {e}")))?;
    let text = match provider {
        // OpenRouter mirrors the OpenAI response schema.
        Provider::OpenAi | Provider::OpenRouter => parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Provider::Anthropic => parsed["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default(),
    };
    if text.trim().is_empty() {
        return Err(StoreError::Other("cleanup returned empty text".to_string()));
    }
    Ok(text.trim().to_string())
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Serializes env-mutating config tests and neutralizes ambient provider
    /// keys so resolution logic is deterministic on any machine.
    pub fn scrub_env() -> MutexGuard<'static, ()> {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY"] {
            std::env::set_var(key, "");
        }
        guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_block_is_inlined_when_present() {
        let db = Store::open(std::path::Path::new(":memory:")).unwrap();
        let prompt = build_user_prompt(
            "and then we shipped it",
            "",
            Some("Yesterday the team…"),
            &db,
        )
        .unwrap();
        assert!(prompt.contains("before the cursor"));
        assert!(prompt.contains("Yesterday the team"));
        // Absent context leaves no block behind.
        let plain = build_user_prompt("hi", "", None, &db).unwrap();
        assert!(!plain.contains("before the cursor"));
    }

    #[test]
    fn prompt_handles_backtracks_and_emoji_requests() {
        assert!(SYSTEM_PROMPT.contains("never mind"));
        assert!(SYSTEM_PROMPT.contains("final intent"));
        assert!(SYSTEM_PROMPT.contains("emoji"));
    }
    use crate::store::Store;

    fn store() -> Store {
        Store::open(std::path::Path::new(":memory:")).unwrap()
    }

    #[test]
    fn openai_body_shape() {
        let db = store();
        let body = build_request_body(Provider::OpenAi, "gpt-x", "um hi", "", None, &db).unwrap();
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "Dictation:\num hi");
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn anthropic_body_shape() {
        let db = store();
        let body = build_request_body(
            Provider::Anthropic,
            "claude-x",
            "um hi",
            "Casual tone",
            None,
            &db,
        )
        .unwrap();
        assert_eq!(body["model"], "claude-x");
        assert_eq!(body["system"], SYSTEM_PROMPT);
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Style instructions: Casual tone"));
        assert!(body.get("temperature").is_some());
    }

    #[test]
    fn user_prompt_includes_dictionary_and_snippets() {
        let db = store();
        db.add_dictionary_term("kubernetes", None).unwrap();
        db.add_snippet("my email", "jon@example.com").unwrap();
        let prompt = build_user_prompt("hi", "", None, &db).unwrap();
        assert!(prompt.contains("Preferred vocabulary: kubernetes"));
        assert!(prompt.contains("“my email”"));
    }

    #[test]
    fn extract_openai_text() {
        let body = r#"{"choices":[{"message":{"content":"  Hello there. "}}]}"#;
        assert_eq!(
            extract_text(Provider::OpenAi, body).unwrap(),
            "Hello there."
        );
    }

    #[test]
    fn extract_anthropic_text_joins_blocks() {
        let body = r#"{"content":[{"type":"text","text":"Hel"},{"type":"text","text":"lo"}]}"#;
        assert_eq!(extract_text(Provider::Anthropic, body).unwrap(), "Hello");
    }

    #[test]
    fn extract_empty_is_error() {
        let body = r#"{"choices":[]}"#;
        assert!(extract_text(Provider::OpenAi, body).is_err());
    }

    #[test]
    fn resolve_config_requires_a_key() {
        let _env = crate::cloud::llm::test_support::scrub_env();
        let db = store();
        // No keys anywhere: explicit provider selection must fail with a
        // helpful error; auto-selection must fail too.
        db.set_setting("llmProvider", &serde_json::json!("openai"))
            .unwrap();
        let err = resolve_config(&db).unwrap_err().to_string();
        assert!(err.contains("openai"));

        let err2 = resolve_config(&store()).unwrap_err().to_string();
        assert!(err2.contains("OpenAI, Claude, or OpenRouter"));
    }

    #[test]
    fn resolve_config_explicit_provider_uses_its_key() {
        let _env = crate::cloud::llm::test_support::scrub_env();
        let db = store();
        db.set_setting("llmProvider", &serde_json::json!("anthropic"))
            .unwrap();
        db.set_setting("anthropicApiKey", &serde_json::json!("sk-ant-test"))
            .unwrap();
        let cfg = resolve_config(&db).unwrap();
        assert_eq!(cfg.provider, Provider::Anthropic);
        assert_eq!(cfg.api_key, "sk-ant-test");
        assert_eq!(cfg.model, DEFAULT_ANTHROPIC_MODEL);
    }

    #[test]
    fn resolve_config_auto_prefers_openai_on_ties() {
        let _env = crate::cloud::llm::test_support::scrub_env();
        let db = store();
        db.set_setting("openaiApiKey", &serde_json::json!("sk-o"))
            .unwrap();
        db.set_setting("anthropicApiKey", &serde_json::json!("sk-a"))
            .unwrap();
        let cfg = resolve_config(&db).unwrap();
        assert_eq!(cfg.provider, Provider::OpenAi);
    }
}

#[cfg(test)]
mod openrouter_tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open(std::path::Path::new(":memory:")).unwrap()
    }

    #[test]
    fn openrouter_body_uses_openai_shape() {
        let db = store();
        let body = build_request_body(Provider::OpenRouter, "m/x", "hi", "", None, &db).unwrap();
        assert_eq!(body["model"], "m/x");
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn openrouter_response_parsed_like_openai() {
        let body = r#"{"choices":[{"message":{"content":"Done."}}]}"#;
        assert_eq!(extract_text(Provider::OpenRouter, body).unwrap(), "Done.");
    }

    #[test]
    fn explicit_openrouter_provider_requires_its_key() {
        let _env = crate::cloud::llm::test_support::scrub_env();
        let db = store();
        db.set_setting("llmProvider", &serde_json::json!("openrouter"))
            .unwrap();
        db.set_setting("openaiApiKey", &serde_json::json!("sk-o"))
            .unwrap();
        let err = resolve_config(&db).unwrap_err().to_string();
        assert!(err.contains("openrouter"));
    }

    #[test]
    fn auto_falls_back_to_openrouter_key() {
        let _env = crate::cloud::llm::test_support::scrub_env();
        let db = store();
        db.set_setting("openrouterApiKey", &serde_json::json!("sk-or"))
            .unwrap();
        let cfg = resolve_config(&db).unwrap();
        assert_eq!(cfg.provider, Provider::OpenRouter);
        assert_eq!(cfg.api_key, "sk-or");
        assert_eq!(cfg.model, DEFAULT_OPENROUTER_MODEL);
    }

    #[test]
    fn auto_prefers_openai_over_others() {
        let _env = crate::cloud::llm::test_support::scrub_env();
        let db = store();
        db.set_setting("anthropicApiKey", &serde_json::json!("a"))
            .unwrap();
        db.set_setting("openrouterApiKey", &serde_json::json!("r"))
            .unwrap();
        db.set_setting("openaiApiKey", &serde_json::json!("o"))
            .unwrap();
        assert_eq!(resolve_config(&db).unwrap().provider, Provider::OpenAi);
    }
}
