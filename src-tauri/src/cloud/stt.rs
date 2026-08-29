use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::store::{Result, Store};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionResult {
    pub text: String,
    pub raw_text: String,
}

// whisper.cpp keeps the model context shared across states. Serializing
// inference avoids competing local decodes when a live preview finishes near
// the end of a session, which is both safer for native backends and kinder to
// CPU-bound machines.
static TRANSCRIPTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// On-device transcription via the configured local engine. `on_delta` fires
/// once with the complete text for this audio segment. The pipeline uses this
/// function for both the final recording and best-effort live phrase previews.
pub fn stream_transcribe(
    db: &Store,
    wav_bytes: &[u8],
    language: &str,
    prompt: Option<&str>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<TranscriptionResult> {
    let _guard = TRANSCRIPTION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = super::local_stt::transcribe_local(db, wav_bytes, language, prompt)?;
    on_delta(&result.text);
    Ok(result)
}

/// Runs one optional preview only when the local recognizer is available.
/// Preview work must never queue behind a final transcription. Returning
/// `None` means another inference already owns the shared recognizer.
pub fn try_stream_transcribe(
    db: &Store,
    wav_bytes: &[u8],
    language: &str,
    prompt: Option<&str>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<Option<TranscriptionResult>> {
    let lock = TRANSCRIPTION_LOCK.get_or_init(|| Mutex::new(()));
    let guard = match lock.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
    };
    let _guard = guard;
    let result = super::local_stt::transcribe_local(db, wav_bytes, language, prompt)?;
    on_delta(&result.text);
    Ok(Some(result))
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
    // Continuity: seed with the tail of the most recent dictation so names,
    // phrasing and topic carry across consecutive sessions.
    if prompt.len() < 420 {
        if let Some(last) = db
            .list_transcripts(1, 0)
            .ok()
            .and_then(|v| v.first().map(|t| t.text.clone()))
        {
            let tail: String = last
                .chars()
                .rev()
                .take(200)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if !prompt.is_empty() && !tail.is_empty() {
                prompt.push(' ');
            }
            prompt.push_str(tail.trim());
        }
    }
    Ok(prompt.trim().to_string())
}
