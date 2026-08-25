use crate::store::{Result, Store};

const MAX_PROMPT_CHARS: usize = 6_000;

pub const SYSTEM_PROMPT: &str = "You clean up raw speech-to-text dictation. Rewrite the dictated text as polished written prose.\n\
Rules:\n\
- Remove filler words (\"um\", \"uh\", \"you know\").\n- When the speaker changes their mind mid-sentence — \"wait\", \"actually\", \"never mind\", \"or rather\", or simply restarting a sentence — keep only the final intent and drop every abandoned word. Example: \"send it to John wait no to Jane tomorrow\" becomes \"Send it to Jane tomorrow.\"\n\
- Add correct punctuation, capitalization, and paragraph breaks.\n\
- Honor spoken formatting commands: \"new line\", \"new paragraph\", \"bullet list\", \"numbered list\".\n\
- Preserve the input language, meaning, tone, and any code-like tokens or file paths.\n\
- Use the preferred vocabulary exactly as given for names and terms.\n- Honor spoken emoji requests: \"insert party emoji\" appends 🎉 at that spot; without such a request, never add emojis.\n\
- Never answer the content, never add information, never comment. Output ONLY the cleaned text.";

/// Cleans up raw dictation entirely on-device via the cleanup-engine
/// sidecar. No network, no API keys.
pub fn polish(
    db: &Store,
    raw_text: &str,
    app_identifier: &str,
    context: Option<&str>,
) -> Result<String> {
    super::local_llm::polish_local(db, raw_text, app_identifier, context)
}

/// Tone instructions for a session: an explicit pill override (styleOverride
/// setting) wins over automatic per-app matching.
pub fn build_style_instructions(
    db: &Store,
    app_identifier: &str,
) -> Result<Option<String>> {
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

pub fn build_user_prompt(
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

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
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

}
