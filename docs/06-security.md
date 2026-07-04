# 06 — Security & Privacy Threat Model

Scope: OpenDictate desktop app, Windows-first, default (rules-only, offline) build.
Method: asset → threat → mitigation, STRIDE-flavored, plus privacy posture.

## Assets

1. **Live microphone audio** — the most sensitive asset; may contain anything.
2. **Transcribed text / dictation history** — durable form of (1).
3. **User clipboard contents** — touched by insertion tier 3.
4. **API keys** (only when cloud rewriter features are compiled + enabled).
5. **Settings / profiles / dictionary** — low sensitivity, but integrity matters
   (a tampered dictionary could rewrite dictated text maliciously).
6. **Update channel** — code-execution vector if compromised.

## Threats & mitigations

### T1 · Covert audio capture (spoofed or buggy "listening" state)
- Session controller is the *single* owner of the capture stream; tray indicator and
  overlay pill are driven by the same state machine — they cannot disagree with the mic.
- Mic stream is closed (not paused) on `Idle`; Windows' own mic-in-use OS indicator
  therefore also reflects truth.
- Push-to-talk mode guarantees capture ⊆ key-held duration.
- OS mic permission requested with a plain-language rationale at onboarding.

### T2 · Audio/text exfiltration
- Default build has **no network code paths**: HTTP client, TLS, and cloud rewriters are
  compile-time features excluded by default; verifiable by dependency audit
  (`cargo tree`) and binary inspection.
- Audio is never written to disk (ring buffer only), except explicit user-enabled debug
  capture with on-screen warning.
- When a cloud rewriter *is* enabled: per-profile allow list (e.g. never for "medical"
  profile), visible indicator on segments sent to cloud, request contains text only —
  never audio, never app/window context beyond what the user configured.

### T3 · Dictation history theft (local attacker / other apps)
- History DB lives in the per-user app-data dir (NTFS ACL = user-only by default).
- History is opt-out entirely, size/age-capped, and one-click purge.
- Encrypt-at-rest via SQLCipher: decision milestone M7 — default-on if perf cost is
  negligible on target hardware; otherwise a setting. (Threat: offline disk access;
  DPAPI-wrapped key so it's bound to the Windows user account.)

### T4 · Clipboard abuse
- Insertion tier 3 snapshots the clipboard, pastes, then restores the snapshot —
  best-effort for non-text formats; delay-render formats from other apps are not
  persisted by us.
- We never *read* the clipboard except during that snapshot/restore window.
- "Clipboard history" feature stores only text *OpenDictate itself inserted* — never a
  general clipboard monitor.
- Clipboard writes marked with `CF_CLIPBOARD_VIEWER_IGNORE`-style hints where supported
  so clipboard-history managers skip transient pastes.

### T5 · API key theft (cloud-enabled builds only)
- Keys stored exclusively in Windows Credential Manager (`keyring` crate), scoped to the
  user account; never in settings JSON, logs, or crash dumps.
- Keys redacted from all `tracing` output by construction (newtype with opaque `Debug`).

### T6 · Settings/profile tampering (integrity)
- Settings/profiles parsed defensively (serde deny-unknown-fields where practical,
  bounds on regex complexity in `UserRules` — regex compiled with size/nesting limits to
  prevent ReDoS via a malicious shared profile).
- Imported profiles (future sharing feature) are treated as untrusted input: no shell
  expansion, no file paths executed, symbol/dictionary entries length-capped.

### T7 · Malicious update / supply chain
- Tauri updater with signature verification (public key pinned in binary); GitHub
  Releases as channel; version-downgrade rejection.
- Model files downloaded over HTTPS with pinned SHA-256 checksums shipped in the app.
- CI runs `cargo audit`/`cargo deny` (advisories + license gates); lockfiles committed.

### T8 · Input-injection misuse (our own SendInput powers)
- Insertion only ever targets the focus captured at hotkey press; no background typing.
- Command executor's destructive ops (`delete previous sentence`, `select all`) act only
  on our own undo-stack spans or require the focused-app context captured in-session.

## Sandboxing & least privilege

- Tauri webview: no filesystem/shell APIs exposed; IPC allow-list is the exact command
  set the UI needs (Tauri v2 capability system).
- Process runs as normal user; no elevation required or requested. (Consequence: cannot
  inject into elevated windows — documented limitation rather than an elevation prompt.)
- Optional Sentry crash reporting is compile-time optional, runtime opt-in, and scrubs
  paths/usernames; minidumps stay local by default.

## Privacy posture (summary)

| Question | Answer |
|---|---|
| Telemetry | None. Opt-in crash reports only, if compiled in. |
| Audio leaves device | Never. |
| Text leaves device | Only if user enables a cloud rewriter, per profile, indicated. |
| Content in logs | Never at default level. |
| History | Local, capped, purgeable, opt-out; encryption decision at M7. |
| Network in default build | Zero code paths (update check only if user enables it). |
