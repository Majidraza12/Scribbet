<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  type ModelStatus = { present: boolean; path: string; size_hint: number };
  type Progress = {
    got: number;
    total: number;
    done: boolean;
    error: string | null;
  };

  let status = $state<ModelStatus | null>(null);
  let downloading = $state(false);
  let got = $state(0);
  let total = $state(0);
  let done = $state(false);
  let error = $state("");

  const pct = $derived(total > 0 ? Math.min(100, (got / total) * 100) : 0);
  const mb = (n: number) => (n / (1024 * 1024)).toFixed(1);

  async function start() {
    error = "";
    downloading = true;
    got = 0;
    total = status?.size_hint ?? 0;
    await invoke("model_download");
  }

  async function restart() {
    await invoke("restart_app");
  }

  onMount(() => {
    invoke<ModelStatus>("model_status").then((s) => {
      status = s;
      done = s.present;
    });
    const unlisten = listen<Progress>("model-progress", (e) => {
      const p = e.payload;
      if (p.error) {
        error = p.error;
        downloading = false;
        return;
      }
      got = p.got;
      total = p.total;
      if (p.done) {
        done = true;
        downloading = false;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div class="page">
  <div class="logo">🎙</div>
  <h1>Welcome to OpenDictate</h1>
  <p class="tagline">Press a hotkey, talk, and clean text lands at your caret — in any app.</p>

  <div class="card">
    <h2>Private by design</h2>
    <ul>
      <li>Speech is transcribed <b>on this PC</b>. Audio and text never leave your device.</li>
      <li>The microphone is live <b>only</b> while you dictate (hotkey held or toggled on) — the tray icon and overlay always show when it's open.</li>
      <li>No account, no cloud, no telemetry.</li>
    </ul>
  </div>

  <div class="card">
    <h2>One-time setup: speech model</h2>
    <p>
      OpenDictate needs the Whisper speech-recognition model
      (≈{status ? mb(status.size_hint) : "60"} MB), downloaded once from Hugging Face and
      verified against a built-in checksum.
    </p>

    {#if done}
      <p class="ok">✓ Model ready.</p>
      <button class="primary" onclick={restart}>Restart OpenDictate</button>
    {:else if downloading}
      <div class="barwrap"><div class="bar" style="width:{pct}%"></div></div>
      <p class="dim">{mb(got)} / {total > 0 ? mb(total) : "…"} MB</p>
    {:else}
      <button class="primary" onclick={start}>Download model</button>
      {#if error}<p class="err">{error}</p>{/if}
    {/if}
  </div>

  <p class="hotkeys">
    Defaults: <span>Ctrl+Shift+Space</span> toggle · <span>Ctrl+Shift+D</span> hold-to-talk
  </p>
</div>

<style>
  /* No :global(body) background — see Settings.svelte; it would paint the
     transparent overlay window. main.ts handles non-overlay page color. */
  .page {
    min-height: 100vh;
    box-sizing: border-box;
    padding: 28px 36px;
    color: #e8e8ee;
    font:
      400 14px/1.55 "Segoe UI Variable",
      "Segoe UI",
      system-ui,
      sans-serif;
    user-select: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .logo {
    font-size: 34px;
  }
  h1 {
    font-size: 22px;
    font-weight: 650;
    margin: 4px 0 2px;
  }
  .tagline {
    color: #8a8a96;
    margin: 0 0 14px;
  }
  .card {
    background: #1b1b22;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 12px;
    padding: 14px 18px;
    margin-bottom: 12px;
  }
  h2 {
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: #3ddc84;
    margin: 0 0 8px;
  }
  ul {
    margin: 0;
    padding-left: 18px;
  }
  li {
    margin: 4px 0;
  }
  p {
    margin: 6px 0;
  }
  .primary {
    background: #3ddc84;
    color: #0d1117;
    border: 0;
    border-radius: 8px;
    padding: 9px 20px;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
    margin-top: 6px;
  }
  .barwrap {
    height: 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
    margin-top: 10px;
  }
  .bar {
    height: 100%;
    background: #3ddc84;
    transition: width 200ms;
  }
  .dim {
    color: #8a8a96;
    font-size: 12.5px;
  }
  .ok {
    color: #3ddc84;
    font-weight: 600;
  }
  .err {
    color: #ff6b6b;
    user-select: text;
  }
  .hotkeys {
    color: #8a8a96;
    font-size: 12.5px;
    text-align: center;
    margin-top: auto;
  }
  .hotkeys span {
    font-family: Consolas, monospace;
    color: #b9b9c6;
    background: #26262f;
    border-radius: 5px;
    padding: 1px 7px;
  }
</style>
