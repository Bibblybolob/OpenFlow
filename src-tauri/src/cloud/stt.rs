use serde::{Deserialize, Serialize};

use crate::store::{Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionResult {
    pub text: String,
    pub raw_text: String,
}

/// Which endpoint performs speech-to-text. OpenAI uses the native
/// `/audio/transcriptions` upload; OpenRouter has no such route, so audio is
/// sent base64-encoded through a chat-completions model with audio input.
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SttProvider {
    OpenAi,
    OpenRouter,
    Local,
}

impl SttProvider {
    fn resolve(db: &Store) -> Self {
        match db
            .get_setting("sttProvider")
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str::<String>(&v).ok())
            .as_deref()
        {
            Some("openrouter") => SttProvider::OpenRouter,
            Some("local") => SttProvider::Local,
            _ => SttProvider::OpenAi,
        }
    }
}

const DEFAULT_OPENAI_STT_MODEL: &str = "gpt-4o-transcribe";
const DEFAULT_OPENROUTER_STT_MODEL: &str = "thinkingmachines/inkling-small:free";

pub fn transcribe(
    db: &Store,
    wav_bytes: &[u8],
    language: &str,
    prompt: Option<&str>,
) -> Result<TranscriptionResult> {
    let provider = SttProvider::resolve(db);
    let model = stt_model(db, provider);
    let text = match provider {
        SttProvider::Local => {
            return super::local_stt::transcribe_local(db, wav_bytes, language, prompt);
        }
        SttProvider::OpenAi => transcribe_openai(db, wav_bytes, &model, language, prompt)?,
        SttProvider::OpenRouter => transcribe_openrouter(db, wav_bytes, &model, language, prompt)?,
    };
    if text.trim().is_empty() {
        return Err(StoreError::Other(
            "transcription came back empty".to_string(),
        ));
    }
    Ok(TranscriptionResult {
        text: text.clone(),
        raw_text: text,
    })
}

fn stt_model(db: &Store, provider: SttProvider) -> String {
    db.get_setting("sttModel")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| match provider {
            SttProvider::OpenAi => DEFAULT_OPENAI_STT_MODEL.to_string(),
            SttProvider::OpenRouter | SttProvider::Local => {
                DEFAULT_OPENROUTER_STT_MODEL.to_string()
            }
        })
}

fn openrouter_key(db: &Store) -> Result<String> {
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    match db.get_setting("openrouterApiKey")? {
        Some(v) => {
            let key: String = serde_json::from_str(&v)?;
            if key.trim().is_empty() {
                Err(StoreError::Other("OpenRouter API key is empty".to_string()))
            } else {
                Ok(key)
            }
        }
        None => Err(StoreError::Other(
            "missing OpenRouter API key — add it in Settings → API keys".to_string(),
        )),
    }
}

fn transcribe_openai(
    db: &Store,
    wav_bytes: &[u8],
    model: &str,
    language: &str,
    prompt: Option<&str>,
) -> Result<String> {
    let api_key = openai_key(db)?;
    if api_key.starts_with("sk-or-") {
        return Err(StoreError::Other(
            "the OpenAI key field holds an OpenRouter key (sk-or-…) — pick \
             \"OpenRouter\" under Transcription in Settings, or paste a real \
             OpenAI key (sk-…)."
                .to_string(),
        ));
    }

    let part = reqwest::blocking::multipart::Part::bytes(wav_bytes.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| StoreError::Other(e.to_string()))?;

    let mut form = reqwest::blocking::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .part("file", part);

    if !language.is_empty() && language != "auto" {
        form = form.text("language", language.to_string());
    }
    if let Some(p) = prompt {
        if !p.trim().is_empty() {
            form = form.text("prompt", p.trim().to_string());
        }
    }

    let resp = super::http_client()?
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(&api_key)
        .multipart(form)
        .send()
        .map_err(|e| StoreError::Other(format!("request failed: {e}")))?;

    let status = resp.status();
    let body = resp.text().map_err(|e| StoreError::Other(e.to_string()))?;
    if !status.is_success() {
        return Err(StoreError::Other(format!(
            "transcription failed ({status}): {body}"
        )));
    }

    #[derive(Deserialize)]
    struct ApiResp {
        text: String,
    }
    serde_json::from_str::<ApiResp>(&body)
        .map(|r| r.text)
        .map_err(|e| StoreError::Other(format!("bad response: {e}")))
}

fn transcribe_openrouter(
    db: &Store,
    wav_bytes: &[u8],
    model: &str,
    language: &str,
    prompt: Option<&str>,
) -> Result<String> {
    let api_key = openrouter_key(db)?;
    let b64 = B64.encode(wav_bytes);

    let mut content =
        String::from("Transcribe this audio verbatim. Output only the transcript, no commentary.");
    if let Some(p) = prompt {
        if !p.trim().is_empty() {
            content.push_str(" Preferred spellings/names, apply where relevant: ");
            content.push_str(p.trim());
        }
    }
    if language != "auto" && !language.is_empty() {
        content.push_str(&format!(" The spoken language is {language}."));
    }

    let body = serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": content},
                {"type": "input_audio", "input_audio": {"data": b64, "format": "wav"}}
            ]
        }]
    });

    let resp = super::http_client()?
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(&api_key)
        .header("HTTP-Referer", "https://flowclone.app")
        .header("X-Title", "FlowClone")
        .json(&body)
        .send()
        .map_err(|e| StoreError::Other(format!("request failed: {e}")))?;

    let status = resp.status();
    let raw = resp.text().map_err(|e| StoreError::Other(e.to_string()))?;
    if !status.is_success() {
        return Err(StoreError::Other(format!(
            "transcription failed ({status}): {raw}"
        )));
    }

    extract_chat_text(&raw)
        .ok_or_else(|| StoreError::Other(format!("unexpected response shape: {raw}")))
}

fn extract_chat_text(raw: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Resp {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
    }
    #[derive(Deserialize)]
    struct Message {
        content: Content,
    }
    // Content may be a plain string or an array of typed parts.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Content {
        Text(String),
        Parts(Vec<Part>),
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Part {
        Text { text: String },
        Other(#[allow(dead_code)] serde_json::Value),
    }

    let resp: Resp = serde_json::from_str(raw).ok()?;
    let msg = resp.choices.into_iter().next()?.message.content;
    match msg {
        Content::Text(t) => Some(t),
        Content::Parts(parts) => {
            let joined = parts
                .into_iter()
                .filter_map(|p| match p {
                    Part::Text { text } => Some(text),
                    Part::Other(_) => None,
                })
                .collect::<Vec<_>>()
                .join("");
            (!joined.is_empty()).then_some(joined)
        }
    }
}

fn openai_key(db: &Store) -> Result<String> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    match db.get_setting("openaiApiKey")? {
        Some(v) => {
            let key: String = serde_json::from_str(&v)?;
            if key.trim().is_empty() {
                Err(StoreError::Other("OpenAI API key is empty".to_string()))
            } else {
                Ok(key)
            }
        }
        None => Err(StoreError::Other(
            "missing OpenAI API key — set OPENAI_API_KEY or add it in Settings".to_string(),
        )),
    }
}

pub fn build_prompt(db: &Store) -> Result<String> {
    let mut prompt = String::new();
    for entry in db.list_dictionary()? {
        if let Some(replacement) = &entry.replacement {
            prompt.push_str(replacement);
        } else {
            prompt.push_str(&entry.term);
        }
        prompt.push(' ');
        if prompt.len() > 400 {
            break;
        }
    }
    Ok(prompt.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_openrouter_key_in_openai_slot() {
        let db = Store::open(std::path::Path::new(":memory:")).unwrap();
        db.set_setting("openaiApiKey", &serde_json::json!("sk-or-v1-test"))
            .unwrap();
        let err = transcribe(&db, b"", "en", None).unwrap_err().to_string();
        assert!(err.contains("OpenRouter"), "unexpected error: {err}");
    }

    #[test]
    fn provider_defaults_to_openai() {
        let db = Store::open(std::path::Path::new(":memory:")).unwrap();
        assert_eq!(SttProvider::resolve(&db), SttProvider::OpenAi);
    }

    #[test]
    fn provider_honors_setting() {
        let db = Store::open(std::path::Path::new(":memory:")).unwrap();
        db.set_setting("sttProvider", &serde_json::json!("openrouter"))
            .unwrap();
        assert_eq!(SttProvider::resolve(&db), SttProvider::OpenRouter);
    }

    #[test]
    fn default_models_follow_provider() {
        let db = Store::open(std::path::Path::new(":memory:")).unwrap();
        assert_eq!(
            stt_model(&db, SttProvider::OpenAi),
            DEFAULT_OPENAI_STT_MODEL
        );
        assert_eq!(
            stt_model(&db, SttProvider::OpenRouter),
            DEFAULT_OPENROUTER_STT_MODEL
        );
    }

    #[test]
    fn parses_plain_and_parted_content() {
        assert_eq!(
            extract_chat_text(r#"{"choices":[{"message":{"content":"hello"}}]}"#).unwrap(),
            "hello"
        );
        assert_eq!(
            extract_chat_text(
                r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"other"}]}}]}"#
            )
            .unwrap(),
            "a"
        );
        assert!(extract_chat_text(r#"{"error":"x"}"#).is_none());
    }
}
