use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store::{Result, Store, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionResult {
    pub text: String,
    pub raw_text: String,
}

pub fn transcribe(
    db: &Store,
    audio: &Path,
    language: &str,
    prompt: Option<&str>,
) -> Result<TranscriptionResult> {
    let api_key = resolve_api_key(db)?;
    let model = db
        .get_setting("sttModel")?
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .unwrap_or_else(|| "gpt-4o-transcribe".to_string());

    let bytes = std::fs::read(audio)?;
    let part = reqwest::blocking::multipart::Part::bytes(bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| StoreError::Other(e.to_string()))?;

    let mut form = reqwest::blocking::multipart::Form::new()
        .text("model", model)
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

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| StoreError::Other(e.to_string()))?;

    let resp = client
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

    let parsed: ApiResp =
        serde_json::from_str(&body).map_err(|e| StoreError::Other(format!("bad response: {e}")))?;

    Ok(TranscriptionResult {
        text: parsed.text.clone(),
        raw_text: parsed.text,
    })
}

fn resolve_api_key(db: &Store) -> Result<String> {
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
