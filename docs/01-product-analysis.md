# 01 — Product Analysis: AI Dictation Category

Survey of the modern AI-dictation category, done to position Scribbet. Analysis is of
publicly observable product behavior only — no proprietary implementation details were
used or copied.

## The category in one paragraph

Classic OS dictation (Windows Speech, Apple Dictation) transcribes literally: you get your
"um"s, your run-on sentences, your missing punctuation. The 2023+ wave (Wispr Flow,
SuperWhisper, Aqua Voice) added two things: **modern STT models** (Whisper-class accuracy
on jargon and accents) and **post-transcription intelligence** (punctuation, filler
removal, tone shaping), delivered through a **hold-a-hotkey, speak, release, text appears
anywhere** interaction. That interaction loop is the product; everything else is plumbing
quality.

## Comparison matrix

| Dimension | Wispr Flow | SuperWhisper | Aqua Voice | MacWhisper | Win Voice Access | Apple Dictation | **Scribbet (target)** |
|---|---|---|---|---|---|---|---|
| Platform | mac/Win | macOS | Web/mac | macOS | Windows | Apple | **Windows → cross-platform** |
| Processing locality | Cloud | Local | Cloud | Local | Local | Local/hybrid | **Local (cloud opt-in rewrite only)** |
| Streaming feel | Yes | Batch-ish | Yes | File-based | Yes | Yes | **Yes (partials in overlay)** |
| Cleanup quality | Excellent (LLM) | Good (modes) | Excellent (LLM) | None (raw STT) | Poor | Poor | **Good (rules) → excellent (opt-in LLM)** |
| Universal insertion | Yes | Yes | Editor-centric | No | Yes | Yes | **Yes (3-tier fallback)** |
| Voice edit commands | Some | No | Yes | No | Extensive | Basic | **Core set, rule grammar** |
| Custom vocabulary | Yes | Yes | Yes | Prompt bias | Limited | No | **Yes (dictionary + STT bias)** |
| Modes/profiles | Tone contexts | Yes | No | No | No | No | **Yes (TOML, user-extensible)** |
| Offline capable | No | Yes | No | Yes | Yes | Partial | **Yes, by default** |
| Open/extensible | No | No | No | No | No | No | **Yes (traits, profiles, plugins)** |
| Business model | Subscription | Paid app | Subscription | Paid app | Free (OS) | Free (OS) | **Owned, no per-word cloud cost** |
| Idle footprint | Heavy (Electron) | Moderate | n/a (web) | Moderate | Light | Light | **<120MB RAM, ~0% CPU target** |

## Strengths worth learning from (patterns, not code)

- **Wispr Flow**: the "it just works everywhere" insertion promise; cleanup so good users
  stop proofreading; push-to-talk as the primary gesture.
- **SuperWhisper**: modes as first-class concept; proof local models are good enough.
- **Aqua Voice**: streaming partial text builds trust — user sees the system heard them.
- **Voice Access**: command grammar depth (navigation, correction) without any LLM.
- **Apple Dictation**: zero-setup; the bar for "no configuration required".

## Weaknesses / opportunities

1. **Privacy vs quality is a forced trade today.** Cloud tools send every utterance off-device;
   local tools ship weak cleanup. A well-built *rules* cleanup chain closes most of the gap
   (fillers, punctuation repair, casing, vocabulary) at zero model cost. → our wedge.
2. **Windows is underserved.** The polished products are macOS-first; Voice Access is
   capable but produces robotic text.
3. **Nothing is extensible.** No user-defined processors, profiles, or replaceable engines
   anywhere in the category.
4. **Footprint ignored.** Electron shells idle at hundreds of MB for a tool that is
   99% idle. A dictation utility should cost almost nothing when silent.
5. **Latency perception is managed, not solved.** Batch tools feel slow even when fast;
   streaming partials + instant final commit is the fix.

## Performance bottlenecks observed in the category

| Bottleneck | Typical cause | Our mitigation |
|---|---|---|
| Long pause → text appears | batch STT after key release | streaming decode during speech; VAD-driven finalization |
| Cloud round-trip on every utterance | LLM rewrite in hot path | rules chain in hot path (<1ms); LLM strictly opt-in |
| High idle RAM | Electron + resident models | Tauri + models mmapped/unloaded when idle |
| First-use delay | model download/load on first dictation | download at onboarding; lazy mmap load on hotkey |
| Insertion failures in odd apps | single insertion strategy | 3-tier fallback + per-app quirk table |

## Non-Goals

Scribbet is a dictation utility, not a platform. Explicitly out of scope — both to
protect the footprint budget and to keep the product legible:

- **Not an AI assistant.** No question answering, no "ask your computer" mode, no agentic
  actions beyond the fixed voice-command grammar. Speech in, text out.
- **Not a meeting transcription platform.** No multi-speaker diarization, no call
  recording, no meeting summaries, no calendar integration. The "meeting" profile formats
  *your own* dictated notes; it does not transcribe other people.
- **Not a chatbot or conversational UI.** No dialogue state, no follow-up turns, no
  personality. The overlay pill shows transcription state, nothing else.
- **Not a note-taking app.** History is a safety net (recover a lost insertion), not a
  knowledge base. No editor, no sync, no organization features — target apps own the
  documents.
- **Not a general voice-control/accessibility suite.** Voice commands cover text editing
  around the dictation flow; OS navigation, window management, and app launching belong
  to Voice Access.
- **Not a translation tool.** Language auto-detect transcribes in the spoken language;
  cross-language translation is out of scope.
- **Not a server or SDK.** One desktop process; no daemon API, no multi-user, no web
  service. (Extensibility = traits, profiles, plugins — inside the app.)

## MVP Success Criteria

v1.0 ships when all of these hold on a mid-range laptop (4-core mobile CPU, no GPU):

| Criterion | Measure | Target |
|---|---|---|
| Perceived latency | speech-end → final text inserted | ≤300 ms p50, ≤500 ms p95 |
| Streaming feedback | speech → partial visible in overlay | ≤500 ms behind voice |
| Cold start | process launch → hotkey live | ≤2 s |
| Idle RAM | working set, 10-min idle soak | ≤120 MB target, 250 MB hard cap |
| Idle CPU | 10-min idle soak average | ≤5% (≈0% expected) |
| Offline | full dictation + commands + profiles with network disabled | 100% functional |
| Universal insertion | manual matrix: Notepad, Word, Chrome, VS Code, Slack, Windows Terminal | text lands correctly in all 6 |
| Voice commands | core set (new paragraph/line, delete previous sentence/word, undo, replace X with Y, select all, paste, stop) | ≥95% recognition on fixture set, zero false triggers on dictation fixtures |
| Cleanup quality | golden-file suite (fillers, punctuation, casing, symbols, dictionary) | 100% of goldens pass |
| Privacy | default build dependency/binary audit | zero network code paths |
| Install size | installer + STT/VAD models | ≤150 MB total |

## Missing features in the category (our additions)

- User-visible latency metrics (trust through transparency).
- Programming-symbol dictation profile that actually works ("open brace", "arrow", "pipe").
- Fully user-defined cleanup rules (regex + replacement, per profile).
- Auditable privacy: default build contains no network code paths at all.
