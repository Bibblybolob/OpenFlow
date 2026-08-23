use std::io::{BufRead, BufReader};

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

/// gpt-4o-mini-transcribe is ~2x faster than the full gpt-4o-transcribe at
/// near-identical dictation quality, which dominates release-to-paste
/// latency for cloud transcription.
const DEFAULT_OPENAI_STT_MODEL: &str = "gpt-4o-mini-transcribe";
const DEFAULT_OPENROUTER_STT_MODEL: &str = "thinkingmachines/inkling-small:free";

/// Streaming transcription: OpenAI's STT endpoint delivers server-sent
/// events as the audio is processed, so `on_delta` receives the cumulative
/// transcript while the request is still in flight — the pill can render
/// text progressively instead of waiting for the whole upload+decode.
/// Providers without a streaming route (local whisper, OpenRouter chat
/// models) simply invoke `on_delta` once with the complete text.
pub fn stream_transcribe(
    db: &Store,
    wav_bytes: &[u8],
    language: &str,
    prompt: Option<&str>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<TranscriptionResult> {
    let provider = SttProvider::resolve(db);
    let model = stt_model(db, provider);
    match provider {
        SttProvider::Local => {
            let result = super::local_stt::transcribe_local(db, wav_bytes, language, prompt);
            emit_single(result, on_delta)
        }
        SttProvider::OpenRouter => {
            let text = transcribe_openrouter(db, wav_bytes, &model, language, prompt);
            emit_single(wrap_text(text), on_delta)
        }
        SttProvider::OpenAi => {
            match transcribe_openai_streaming(db, wav_bytes, &model, language, prompt, on_delta) {
                Ok(result) => Ok(result),
                Err(e) => {
                    // Models that reject `stream` (e.g. whisper-1), proxies that
                    // mangle SSE, transient mid-stream failures — never lose the
                    // dictation over it.
                    eprintln!("streaming transcription failed ({e}) — falling back to batch");
                    let text = transcribe_openai(db, wav_bytes, &model, language, prompt);
                    emit_single(wrap_text(text), on_delta)
                }
            }
        }
    }
}

fn wrap_text(result: Result<String>) -> Result<TranscriptionResult> {
    result.map(|text| TranscriptionResult {
        raw_text: text.clone(),
        text,
    })
}

fn emit_single(
    result: Result<TranscriptionResult>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<TranscriptionResult> {
    let result = result?;
    on_delta(&result.text);
    Ok(result)
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

fn transcribe_openai_streaming(
    db: &Store,
    wav_bytes: &[u8],
    model: &str,
    language: &str,
    prompt: Option<&str>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<TranscriptionResult> {
    let api_key = openai_key(db)?;
    if api_key.starts_with("sk-or-") {
        return Err(StoreError::Other(
            "the OpenAI key field holds an OpenRouter key (sk-or-…)".to_string(),
        ));
    }

    let part = reqwest::blocking::multipart::Part::bytes(wav_bytes.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| StoreError::Other(e.to_string()))?;

    let mut form = reqwest::blocking::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .text("stream", "true")
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
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let head: String = body.chars().take(200).collect();
        return Err(StoreError::Other(format!(
            "transcription failed ({status}): {head}"
        )));
    }

    // Read the SSE body line-by-line as it arrives over the wire; the
    // blocking reader yields each event as soon as the server flushes it.
    let mut reader = BufReader::new(resp);
    let mut line = String::new();
    let mut cumulative = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| StoreError::Other(format!("stream read failed: {e}")))?;
        if read == 0 {
            break;
        }
        match parse_stt_sse_line(&line) {
            SttSseEvent::Delta(d) => {
                cumulative.push_str(&d);
                on_delta(&cumulative);
            }
            // The done event's full text is authoritative — prefer it over
            // whatever deltas accumulated.
            SttSseEvent::Done(full) => {
                if let Some(text) = full.filter(|t| !t.trim().is_empty()) {
                    if text != cumulative {
                        on_delta(&text);
                    }
                    cumulative = text;
                }
                break;
            }
            SttSseEvent::Ignore => {}
        }
    }

    if cumulative.trim().is_empty() {
        return Err(StoreError::Other(
            "streaming produced no transcript".to_string(),
        ));
    }
    Ok(TranscriptionResult {
        text: cumulative.clone(),
        raw_text: cumulative,
    })
}

/// One parsed server-sent event from the STT stream.
#[derive(Debug, PartialEq)]
enum SttSseEvent {
    /// An incremental chunk of transcript.
    Delta(String),
    /// End of stream; optionally carrying the authoritative full transcript.
    Done(Option<String>),
    /// Comments, keepalives, unknown event types.
    Ignore,
}

fn parse_stt_sse_line(line: &str) -> SttSseEvent {
    let trimmed = line.trim();
    let Some(payload) = trimmed.strip_prefix("data:") else {
        return SttSseEvent::Ignore;
    };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return SttSseEvent::Done(None);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return SttSseEvent::Ignore;
    };
    match value["type"].as_str() {
        Some("transcript.text.delta") => match value["delta"].as_str() {
            Some(d) if !d.is_empty() => SttSseEvent::Delta(d.to_string()),
            _ => SttSseEvent::Ignore,
        },
        Some("transcript.text.done") => {
            SttSseEvent::Done(value["text"].as_str().map(std::string::ToString::to_string))
        }
        _ => SttSseEvent::Ignore,
    }
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
        let err = stream_transcribe(&db, b"", "en", None, &mut |_| {})
            .unwrap_err()
            .to_string();
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
    fn sse_parser_handles_delta_done_and_noise() {
        use SttSseEvent::*;
        let delta_line = "data: {\"type\":\"transcript.text.delta\",\"delta\":\"Hello \"}";
        assert_eq!(parse_stt_sse_line(delta_line), Delta("Hello ".to_string()));
        let done_line = "data: {\"type\":\"transcript.text.done\",\"text\":\"Hello world.\"}";
        assert_eq!(
            parse_stt_sse_line(done_line),
            Done(Some("Hello world.".to_string()))
        );
        assert!(matches!(parse_stt_sse_line("data: [DONE]"), Done(None)));
        assert!(matches!(parse_stt_sse_line(": keepalive"), Ignore));
        assert!(matches!(
            parse_stt_sse_line("data: {\"type\":\"transcript.segment\"}"),
            Ignore
        ));
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
