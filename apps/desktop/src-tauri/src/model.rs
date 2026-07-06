//! STT model onboarding (M8): user-initiated download with resume and a
//! pinned SHA-256, mirroring scripts/fetch-models.ps1.
//!
//! This is the app's single deliberate network code path besides nothing
//! (docs/06 "Network in default build"): it talks only to the pinned URL,
//! only when the user clicks Download in the onboarding window, and the
//! file is rejected unless its checksum matches the constant below.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

/// whisper.cpp base.en Q5_1 — the v1 model (ADR-4).
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin";
/// Pinned SHA-256 of the file above (same pin as scripts/fetch-models.ps1).
const MODEL_SHA256: &str = "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f";
/// Expected size, bytes (progress display before the server answers).
const MODEL_SIZE_HINT: u64 = 59_707_625;

/// Progress event payload (`model-progress` on the onboarding window).
#[derive(Clone, Serialize)]
struct Progress {
    got: u64,
    total: u64,
    done: bool,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct ModelStatus {
    pub present: bool,
    pub path: String,
    pub size_hint: u64,
}

pub fn status() -> ModelStatus {
    let path = od_stt::default_model_path();
    ModelStatus {
        present: path.is_file(),
        path: path.display().to_string(),
        size_hint: MODEL_SIZE_HINT,
    }
}

/// Starts the download on a worker thread; progress and completion stream
/// back as `model-progress` events. A second call while one is running is
/// prevented by the frontend (button disabled while in flight).
pub fn spawn_download(app: AppHandle) {
    std::thread::Builder::new()
        .name("od-model-download".into())
        .spawn(move || {
            let result = download(&app);
            let payload = match result {
                Ok(total) => Progress {
                    got: total,
                    total,
                    done: true,
                    error: None,
                },
                Err(e) => {
                    tracing::error!("model download failed: {e}");
                    Progress {
                        got: 0,
                        total: 0,
                        done: false,
                        error: Some(e),
                    }
                }
            };
            let _ = app.emit("model-progress", &payload);
        })
        .expect("spawn model download thread");
}

fn emit_progress(app: &AppHandle, got: u64, total: u64) {
    let _ = app.emit(
        "model-progress",
        &Progress {
            got,
            total,
            done: false,
            error: None,
        },
    );
}

/// Downloads to `<model>.part` (resuming if a partial file exists), verifies
/// the pinned checksum, and renames into place. Returns the final size.
fn download(app: &AppHandle) -> Result<u64, String> {
    let final_path = od_stt::default_model_path();
    let dir = final_path
        .parent()
        .ok_or("model path has no parent directory")?;
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let part: PathBuf = final_path.with_extension("bin.part");

    let mut have: u64 = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    let mut req = ureq::get(MODEL_URL);
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| format!("request failed: {e}"))?;

    // 206 = server honored the resume; 200 = full body, start over.
    let (total, append) = match resp.status() {
        206 => {
            let total = resp
                .header("Content-Range")
                .and_then(|cr| cr.rsplit('/').next())
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(MODEL_SIZE_HINT);
            (total, true)
        }
        200 => {
            have = 0;
            let total = resp
                .header("Content-Length")
                .and_then(|l| l.parse::<u64>().ok())
                .unwrap_or(MODEL_SIZE_HINT);
            (total, false)
        }
        s => return Err(format!("unexpected HTTP status {s}")),
    };

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(append)
        .write(true)
        .truncate(!append)
        .open(&part)
        .map_err(|e| format!("open {}: {e}", part.display()))?;

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut since_emit: u64 = 0;
    emit_progress(app, have, total);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write: {e}"))?;
        have += n as u64;
        since_emit += n as u64;
        if since_emit >= 1024 * 1024 {
            since_emit = 0;
            emit_progress(app, have, total);
        }
    }
    file.flush().map_err(|e| format!("flush: {e}"))?;
    drop(file);

    // Verify the pinned checksum before the file may carry the real name.
    let sha = sha256_file(&part)?;
    if sha != MODEL_SHA256 {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "checksum mismatch (got {sha}); download discarded — please retry"
        ));
    }
    fs::rename(&part, &final_path).map_err(|e| format!("rename into place: {e}"))?;
    tracing::info!(path = %final_path.display(), "model downloaded and verified");
    Ok(have)
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
