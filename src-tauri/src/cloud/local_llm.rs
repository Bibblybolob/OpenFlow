//! On-device cleanup LLM. The GGUF model runs in a separate `cleanup-engine`
//! process because llama.cpp cannot be linked into this app directly:
//! whisper.cpp (via whisper-rs) vendors its own ggml, and merging both
//! symbol tables corrupts llama.cpp's log-callback state, segfaulting model
//! load (utilityai/llama-cpp-rs#263). The sidecar links llama.cpp alone,
//! loads the model once, and answers JSON-line requests over stdio.

use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::OnceLock;

use serde_json::json;
use tauri::Emitter;

use crate::store::{Result, Store, StoreError};

pub struct LocalLlm {
    pub id: &'static str,
    pub label: &'static str,
    /// Approximate download size in MB, for UI display.
    pub approx_mb: u64,
}

/// Both models speak ChatML (`<|im_start|>` turns), so one prompt template
/// serves the catalog. Qwen3-4B punches far above its size for the cleanup
/// task; LFM2 trades accuracy for speed on weaker hardware.
pub const LOCAL_LLMS: &[LocalLlm] = &[
    LocalLlm {
        id: "qwen3-4b",
        label: "Qwen3 4B — best offline cleanup (recommended)",
        approx_mb: 2400,
    },
    LocalLlm {
        id: "lfm2-1b",
        label: "LFM2 1.2B — fastest, lower quality",
        approx_mb: 760,
    },
];

const DEFAULT_LOCAL_LLM_ID: &str = "qwen3-4b";

pub(crate) fn catalog_llm(id: &str) -> Option<&'static LocalLlm> {
    LOCAL_LLMS.iter().find(|m| m.id == id)
}

/// Resolves the configured local cleanup model id against the catalog,
/// defaulting to the recommendation. Stored under its own key so it never
/// collides with the cloud `llmModel` override.
pub fn resolve_llm_id(db: &Store) -> String {
    db.get_setting("llmLocalModel")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .filter(|m| catalog_llm(m).is_some())
        .unwrap_or_else(|| DEFAULT_LOCAL_LLM_ID.to_string())
}

/// File name on Hugging Face for each catalog id.
fn gguf_file_name(id: &str) -> &'static str {
    match id {
        "qwen3-4b" => "qwen3-4b-instruct-2507-q4_k_m.gguf",
        "lfm2-1b" => "LFM2-1.2B-Q4_K_M.gguf",
        _ => "unknown.gguf",
    }
}

fn hf_url(id: &str) -> &'static str {
    match id {
        // Qwen's own GGUF repo is gated; bartowski's quantization mirror is open.
        "qwen3-4b" => "https://huggingface.co/bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        "lfm2-1b" => "https://huggingface.co/LiquidAI/LFM2-1.2B-GGUF/resolve/main/LFM2-1.2B-Q4_K_M.gguf",
        _ => "",
    }
}

pub fn llm_path(id: &str) -> PathBuf {
    super::local_stt::models_dir().join(gguf_file_name(id))
}

pub fn is_downloaded(id: &str) -> bool {
    llm_path(id).try_exists().unwrap_or(false)
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    model: String,
    downloaded_mb: u64,
    total_mb: u64,
}

/// Streams a GGUF to disk, emitting `local-llm-progress` events for the Hub.
pub fn download_model(app: &tauri::AppHandle, id: &str) -> Result<PathBuf> {
    let Some(entry) = catalog_llm(id) else {
        return Err(StoreError::Other(format!("unknown local LLM: {id}")));
    };
    let dest = llm_path(id);
    if is_downloaded(id) {
        return Ok(dest);
    }

    let mut resp = super::http_client()?
        .get(hf_url(id))
        .send()
        .map_err(|e| StoreError::Other(format!("model download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(StoreError::Other(format!(
            "model download failed ({})",
            resp.status()
        )));
    }

    std::fs::create_dir_all(super::local_stt::models_dir())?;
    let partial = dest.with_extension("part");
    let mut file = File::create(&partial)?;

    let total = resp
        .content_length()
        .unwrap_or(entry.approx_mb * 1024 * 1024);
    let mut downloaded: usize = 0;
    let mut last_emit_mb: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| StoreError::Other(format!("model download failed: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n;

        let mb = (downloaded / (1024 * 1024)) as u64;
        if mb > last_emit_mb {
            last_emit_mb = mb;
            let _ = app.emit(
                "local-llm-progress",
                DownloadProgress {
                    model: id.to_string(),
                    downloaded_mb: mb,
                    total_mb: total / (1024 * 1024),
                },
            );
        }
    }
    file.flush()?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    drop(file);
    if size < 50 * 1024 * 1024 {
        let _ = std::fs::remove_file(&dest);
        return Err(StoreError::Other(
            "downloaded file looks truncated — please retry".to_string(),
        ));
    }

    std::fs::rename(&partial, &dest)?;
    let _ = app.emit("local-llm-progress", json!({ "type": "done", "model": id }));
    Ok(dest)
}

// ---------------------------------------------------------------------------
// Sidecar plumbing
// ---------------------------------------------------------------------------

const GPU_LAYERS: u32 = 99;

struct EngineProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    loaded_path: Option<String>,
}

static ENGINE: OnceLock<Mutex<Option<EngineProcess>>> = OnceLock::new();

impl Drop for EngineProcess {
    fn drop(&mut self) {
        // Dropping a Child handle does not stop the process; without this,
        // every crash-retry respawns a new engine while the old one leaks.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn engine_slot() -> &'static Mutex<Option<EngineProcess>> {
    ENGINE.get_or_init(|| Mutex::new(None))
}

/// Locates the bundled cleanup-engine executable. In production it sits next
/// to the app binary; under cargo test we climb out of deps/.
fn engine_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| StoreError::Other(format!("cannot locate app binary: {e}")))?;
    let mut dir = exe.parent().map(PathBuf::from);
    while let Some(d) = dir {
        // Windows builds carry a .exe suffix; macOS/Linux use the bare name.
        for name in ["cleanup-engine.exe", "cleanup-engine"] {
            let candidate = d.join(name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        dir = d.parent().map(PathBuf::from);
    }
    Err(StoreError::Other(
        "cleanup-engine helper missing from app bundle".to_string(),
    ))
}

fn spawn_engine() -> Result<EngineProcess> {
    let binary = engine_binary()?;
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| StoreError::Other(format!("failed to start cleanup engine: {e}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| StoreError::Other("engine stdin unavailable".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| StoreError::Other("engine stdout unavailable".to_string()))?;
    Ok(EngineProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
        loaded_path: None,
    })
}

impl EngineProcess {
    /// Sends one request and reads its reply. The engine replies exactly
    /// once per request line, so a plain read_line pairs them up.
    fn request(
        &mut self,
        payload: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        self.next_id += 1;
        let mut req = payload;
        req["id"] = json!(self.next_id);
        let line = req.to_string();
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("engine pipe broken: {e}"))?;

        let mut reply = String::new();
        let n = self
            .stdout
            .read_line(&mut reply)
            .map_err(|e| format!("engine pipe broken: {e}"))?;
        if n == 0 {
            return Err("cleanup engine exited unexpectedly".to_string());
        }
        if !reply.starts_with('{') {
            // The engine's stderr (tracing) must never reach stdout; if it
            // does, surface the stray line instead of a cryptic parse error.
            return Err(format!(
                "unexpected engine output: {}",
                reply.chars().take(200).collect::<String>()
            ));
        }
        serde_json::from_str(&reply).map_err(|e| format!("bad engine reply: {e}"))
    }

    fn ensure_loaded(&mut self, path: &Path) -> std::result::Result<(), String> {
        let path_str = path.to_string_lossy().into_owned();
        if self.loaded_path.as_deref() == Some(path_str.as_str()) {
            return Ok(());
        }
        let reply = self.request(json!({
            "op": "load",
            "path": path_str,
            "gpuLayers": GPU_LAYERS,
        }))?;
        if reply.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            return Err(reply
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("load failed")
                .to_string());
        }
        self.loaded_path = Some(path_str);
        Ok(())
    }
}

/// Ensures an engine process exists and has the requested model loaded.
/// Respawns once if the previous process died mid-flight.
fn with_engine<T>(
    path: &Path,
    f: impl Fn(&mut EngineProcess) -> std::result::Result<T, String>,
) -> Result<T> {
    let slot = engine_slot();
    let mut guard = slot.lock().unwrap();
    if guard.is_none() {
        *guard = Some(spawn_engine()?);
    }
    let engine = guard.as_mut().expect("just ensured");
    match engine.ensure_loaded(path).and_then(|_| f(engine)) {
        Ok(v) => Ok(v),
        Err(_first_err) => {
            // One retry on a fresh process covers engine crashes.
            let _ = guard.take();
            *guard = Some(spawn_engine()?);
            let engine = guard.as_mut().expect("just respawned");
            engine
                .ensure_loaded(path)
                .and_then(|_| f(engine))
                .map_err(StoreError::Other)
        }
    }
}

// ---------------------------------------------------------------------------
// Prompting + public API
// ---------------------------------------------------------------------------

fn wrap_chatml(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n\
         <|im_start|>user\n{user}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

/// Cleans up raw dictation entirely on-device. Prompt assembly (style
/// instructions, dictionary, snippets, context) reuses the cloud builders.
pub fn polish_local(
    db: &Store,
    raw_text: &str,
    app_identifier: &str,
    context: Option<&str>,
) -> Result<String> {
    let style = super::llm::build_style_instructions(db, app_identifier)?;
    let user_prompt =
        super::llm::build_user_prompt(raw_text, &style.unwrap_or_default(), context, db)?;
    let prompt = wrap_chatml(super::llm::SYSTEM_PROMPT, &user_prompt);

    let id = resolve_llm_id(db);
    let path = llm_path(&id);
    if !is_downloaded(&id) {
        return Err(StoreError::Other(format!(
            "on-device cleanup model \"{id}\" not downloaded yet — get it in Settings → Cleanup"
        )));
    }

    let started = std::time::Instant::now();
    let text = with_engine(&path, |engine| {
        let reply = engine.request(json!({ "op": "cleanup", "prompt": prompt }))?;
        if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(reply
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string())
        } else {
            Err(reply
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("cleanup failed")
                .to_string())
        }
    })?;

    eprintln!("local llm \"{id}\" cleaned up in {:?}", started.elapsed());
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(StoreError::Other("cleanup returned empty text".to_string()));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_resolve_and_paths_match_filenames() {
        let db = Store::open(std::path::Path::new(":memory:")).unwrap();
        assert_eq!(resolve_llm_id(&db), "qwen3-4b");
        db.set_setting("llmLocalModel", &serde_json::json!("lfm2-1b"))
            .unwrap();
        assert_eq!(resolve_llm_id(&db), "lfm2-1b");
        // Unknown ids fall back to the default rather than erroring.
        db.set_setting("llmLocalModel", &serde_json::json!("gpt-4o"))
            .unwrap();
        assert_eq!(resolve_llm_id(&db), "qwen3-4b");

        assert!(llm_path("qwen3-4b")
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".gguf"));
    }

    #[test]
    fn chatml_wraps_system_and_user_turns() {
        let p = wrap_chatml("SYS", "USER");
        assert!(p.starts_with("<|im_start|>system\nSYS<|im_end|>"));
        assert!(p.contains("<|im_start|>user\nUSER<|im_end|>"));
        assert!(p.ends_with("<|im_start|>assistant\n"));
    }
}

/// Manual hardware gate: requires the qwen3-4b gguf and a built
/// cleanup-engine binary (`cargo build`). Exercises the real sidecar path.
///   cargo test local_llm_cleans_up_real_dictation -- --ignored --nocapture
#[test]
#[ignore = "manual: requires a downloaded local cleanup model"]
fn local_llm_cleans_up_real_dictation() {
    // Default per-platform install layouts; override with FLOWCLONE_MODELS_DIR.
    let dir = std::env::var("FLOWCLONE_MODELS_DIR").map_or_else(
        |_| {
            #[cfg(target_os = "macos")]
            {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join("Library/Application Support/com.flowclone.app/models")
            }
            #[cfg(not(target_os = "macos"))]
            {
                std::env::var("APPDATA")
                    .map(|v| PathBuf::from(v).join("com.flowclone.app/models"))
                    .unwrap_or_else(|_| std::env::temp_dir().join("flowclone-models"))
            }
        },
        PathBuf::from,
    );
    super::local_stt::init_models_dir(dir);
    assert!(is_downloaded("qwen3-4b"), "qwen3-4b gguf missing on disk");
    let db = Store::open(std::path::Path::new(":memory:")).unwrap();
    db.set_setting("llmProvider", &serde_json::json!("local"))
        .unwrap();
    db.set_setting("llmLocalModel", &serde_json::json!("qwen3-4b"))
        .unwrap();
    let raw = "um so basically uh send the report to John wait no to Jane tomorrow \
               and um you know also new line thanks for the update";
    let started = std::time::Instant::now();
    let out =
        polish_local(&db, raw, "com.example.app", None).expect("local cleanup via engine sidecar");
    eprintln!("gate took {:?}, cleaned: {out}", started.elapsed());
    assert!(!out.trim().is_empty());
}
