//! Standalone cleanup engine. Runs llama.cpp in its own process because
//! llama.cpp and whisper.cpp (linked into the main app) cannot coexist in
//! one binary: both vendor ggml, and the merged symbol table corrupts
//! llama.cpp's log-callback state, segfaulting during model load (see
//! utilityai/llama-cpp-rs#263). As a separate process there is no conflict.
//!
//! Protocol on stdin/stdout, one JSON object per line:
//!   → {"id":1, "op":"load",   "path":"...", "gpuLayers":99}
//!   → {"id":2, "op":"cleanup","prompt":"<full chatml prompt>"}
//!   → {"id":3, "op":"shutdown"}
//!   ← {"id":N, "ok":true,  ...}  or  {"id":N, "ok":false, "error":"..."}
//!
//! The process stays alive so the model loads once and serves every
//! dictation; the app kills it on quit.

use std::io::{BufRead as _, Write as _};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};

struct Engine {
    backend: LlamaBackend,
    model: Option<LlamaModel>,
}

static ENGINE: OnceLock<std::sync::Mutex<Engine>> = OnceLock::new();

fn engine() -> &'static std::sync::Mutex<Engine> {
    ENGINE.get_or_init(|| {
        let mut backend = LlamaBackend::init().expect("backend init failed");
        // Route llama.cpp logs through the safe tracing bridge instead of
        // the default stderr printer (which has crashed in some builds).
        // The sidecar's own diagnostics go to stderr below.
        llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
        std::sync::Mutex::new(Engine {
            backend,
            model: None,
        })
    })
}

fn main() {
    // llama.cpp logs go to stderr via tracing. Never stdout: the app
    // parses stdout as JSON lines.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .init();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            // Parent closed the pipe. Exit without C++ static destructors —
            // ggml's Metal teardown asserts on this macOS build.
            std::process::exit(0);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({"ok": false, "error": format!("bad request: {e}")})
                );
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::json!(0));
        let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("");

        let resp = match op {
            "load" => handle_load(&req),
            "cleanup" => handle_cleanup(&req),
            "shutdown" => break,
            other => Err(format!("unknown op: {other}")),
        };

        let out = match resp {
            Ok(mut v) => {
                v["id"] = id;
                v
            }
            Err(e) => serde_json::json!({"id": id, "ok": false, "error": e}),
        };
        let _ = writeln!(stdout, "{out}");
        let _ = stdout.flush();
        if op == "shutdown" {
            // Skip C++ static destructors: ggml's Metal teardown asserts on
            // this macOS build even though everything is already flushed.
            std::process::exit(0);
        }
    }
}

fn handle_load(req: &serde_json::Value) -> Result<serde_json::Value, String> {
    let path = req
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("missing path")?;
    let gpu_layers = req.get("gpuLayers").and_then(|v| v.as_u64()).unwrap_or(99) as u32;

    let mut guard = engine().lock().map_err(|e| e.to_string())?;
    // Reload only when the path actually changed.
    if guard.model.is_none() {
        let params = LlamaModelParams::default()
            .with_n_gpu_layers(gpu_layers)
            .with_use_mmap(false);
        let model = LlamaModel::load_from_file(&guard.backend, Path::new(path), &params)
            .map_err(|e| format!("failed to load model: {e}"))?;
        guard.model = Some(model);
    }
    Ok(serde_json::json!({"ok": true}))
}

const MAX_NEW_TOKENS: usize = 700;
const N_THREADS: i32 = 4;

fn handle_cleanup(req: &serde_json::Value) -> Result<serde_json::Value, String> {
    let prompt = req
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or("missing prompt")?;

    let mut guard = engine().lock().map_err(|e| e.to_string())?;
    let model = guard.model.as_ref().ok_or("no model loaded")?;

    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("tokenize failed: {e}"))?;

    let wanted = u32::try_from(tokens.len() + MAX_NEW_TOKENS + 128).unwrap_or(u32::MAX);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(wanted.min(8192)))
        .with_n_threads(N_THREADS)
        // Single-sequence inference needs one unified KV stream. Without
        // this the multi-stream cache never consumes our batch (all tokens
        // are seq 0) and every decode fails with n_tokens == 0.
        .with_kv_unified(true);
    let mut ctx = model
        .new_context(&guard.backend, ctx_params)
        .map_err(|e| format!("context failed: {e}"))?;
    eprintln!(
        "engine: prompt {} chars -> {} tokens",
        prompt.len(),
        tokens.len()
    );

    let mut batch = LlamaBatch::new(tokens.len(), 1);
    let last_index = tokens.len() - 1;
    for (i, token) in tokens.iter().enumerate() {
        let pos = i32::try_from(i).map_err(|_| "prompt too long")?;
        batch
            .add(*token, pos, &[0], i == last_index)
            .map_err(|e| format!("batch failed: {e}"))?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| format!("decode failed: {e}"))?;

    let mut out = String::new();
    for step in 0..MAX_NEW_TOKENS {
        let best = ctx
            .candidates()
            .max_by(|a, b| a.logit().total_cmp(&b.logit()))
            .map(|c| c.id())
            .ok_or("no candidates")?;
        if model.is_eog_token(best) {
            break;
        }
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        out.push_str(
            &model
                .token_to_piece(best, &mut decoder, true, None)
                .map_err(|e| format!("detokenize failed: {e}"))?,
        );

        let next_pos = i32::try_from(tokens.len() + step).map_err(|_| "position overflow")?;
        batch.clear();
        batch
            .add(best, next_pos, &[0], true)
            .map_err(|e| format!("batch failed: {e}"))?;
        ctx.decode(&mut batch)
            .map_err(|e| format!("decode failed: {e}"))?;
    }

    Ok(serde_json::json!({"ok": true, "text": out.trim()}))
}
