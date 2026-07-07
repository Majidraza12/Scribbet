<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";

  /** Mirrors od_storage::Settings. */
  type Settings = {
    active_profile: string;
    input_device: string | null;
    hotkey_toggle: string;
    hotkey_ptt: string;
    history_enabled: boolean;
    history_cap: number;
  };
  type ProfileInfo = { id: string; name: string };
  type DeviceInfo = { name: string; is_default: boolean };
  type DictEntry = { spoken: string; written: string; case_sensitive: boolean };
  type HistoryEntry = {
    id: number;
    ts_ms: number;
    raw: string;
    cleaned: string;
    profile: string;
  };
  type Perf = {
    cold_start_ms: number;
    finalize_ms_last: number | null;
    finalize_ms_best: number | null;
    insert_ms_last: number | null;
    insert_tier_last: string | null;
    utterances: number;
    inserts: number;
    insert_failures: number;
  };
  /** Mirrors od_storage::ProfileToml (JSON-shaped). */
  type ProfileToml = {
    profile: { name: string };
    stt: { language: string; vocab_bias: string[] };
    cleanup: {
      whitespace: { enabled: boolean };
      fillers: { enabled: boolean; extra: string[] };
      dictionary: { enabled: boolean };
      symbols: { enabled: boolean; table: string };
      punctuation: { enabled: boolean; spoken: boolean; ensure_terminal: boolean };
      segmentation: { enabled: boolean };
      capitalization: { enabled: boolean };
      proper_nouns: string[];
      rules: { pattern: string; replacement: string }[];
    };
    dictionaries: { sets: string[] };
    format: { casing_commands: boolean; email_layout: boolean; bullets: boolean };
    cloud: { rewriter_allowed: boolean };
    plugins?: unknown;
  };

  const tabs = ["General", "Cleanup", "Dictionary", "History", "Performance"] as const;
  let tab = $state<(typeof tabs)[number]>("General");

  let settings = $state<Settings | null>(null);
  let profiles = $state<ProfileInfo[]>([]);
  let devices = $state<DeviceInfo[]>([]);
  let profileId = $state("");
  let profile = $state<ProfileToml | null>(null);
  let dict = $state<DictEntry[]>([]);
  let history = $state<HistoryEntry[]>([]);
  let perf = $state<Perf | null>(null);
  let status = $state("");
  let statusIsError = $state(false);
  let statusTimer: ReturnType<typeof setTimeout> | undefined;

  function flash(msg: string, isError = false) {
    status = msg;
    statusIsError = isError;
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => (status = ""), isError ? 6000 : 2000);
  }

  async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | undefined> {
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      flash(String(e), true);
      return undefined;
    }
  }

  async function loadAll() {
    settings = (await call<Settings>("get_settings")) ?? settings;
    profiles = (await call<ProfileInfo[]>("list_profiles")) ?? [];
    devices = (await call<DeviceInfo[]>("list_devices")) ?? [];
    const active = await call<[string, ProfileToml]>("get_active_profile");
    if (active) [profileId, profile] = active;
    dict = (await call<DictEntry[]>("dict_list")) ?? [];
    history = (await call<HistoryEntry[]>("history_list", { limit: 200 })) ?? [];
    perf = (await call<Perf>("get_perf")) ?? perf;
  }

  async function saveSettings() {
    if (!settings) return;
    const ok = await call("update_settings", { new: $state.snapshot(settings) });
    if (ok !== undefined) flash("Saved");
  }

  async function onProfileSelect(id: string) {
    if (!settings) return;
    settings.active_profile = id;
    await saveSettings();
    const active = await call<[string, ProfileToml]>("get_active_profile");
    if (active) [profileId, profile] = active;
  }

  async function saveProfile() {
    if (!profile) return;
    const ok = await call("save_profile", {
      id: profileId,
      profile: $state.snapshot(profile),
    });
    if (ok !== undefined) flash(`Profile "${profile.profile.name}" saved`);
  }

  // ------- hotkey capture -------
  let capturing = $state<"toggle" | "ptt" | null>(null);

  function hotkeyFromEvent(e: KeyboardEvent): string | null {
    const mods: string[] = [];
    if (e.ctrlKey) mods.push("ctrl");
    if (e.shiftKey) mods.push("shift");
    if (e.altKey) mods.push("alt");
    if (e.metaKey) mods.push("super");
    const code = e.code;
    if (/^(Control|Shift|Alt|Meta)/.test(code)) return null; // modifier alone
    let key: string;
    if (code.startsWith("Key")) key = code.slice(3).toLowerCase();
    else if (code.startsWith("Digit")) key = code.slice(5);
    else if (code === "Space") key = "space";
    else key = code; // F1..F24, Home, etc. — tauri accepts these names
    if (mods.length === 0) return null; // global hotkeys need a modifier
    return [...mods, key].join("+");
  }

  async function onCaptureKey(e: KeyboardEvent) {
    if (!capturing || !settings) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") {
      capturing = null;
      return;
    }
    const combo = hotkeyFromEvent(e);
    if (!combo) return;
    if (capturing === "toggle") settings.hotkey_toggle = combo;
    else settings.hotkey_ptt = combo;
    capturing = null;
    await saveSettings();
  }

  // ------- dictionary -------
  let newSpoken = $state("");
  let newWritten = $state("");
  let newCase = $state(false);

  async function addEntry() {
    if (!newSpoken.trim() || !newWritten.trim()) return;
    const ok = await call("dict_add", {
      entry: { spoken: newSpoken, written: newWritten, case_sensitive: newCase },
    });
    if (ok !== undefined) {
      newSpoken = "";
      newWritten = "";
      newCase = false;
      dict = (await call<DictEntry[]>("dict_list")) ?? dict;
      flash("Entry saved");
    }
  }

  async function removeEntry(spoken: string) {
    await call("dict_remove", { spoken });
    dict = (await call<DictEntry[]>("dict_list")) ?? dict;
  }

  // ------- history -------
  let historyFilter = $state("");
  const historyShown = $derived(
    historyFilter.trim().length === 0
      ? history
      : history.filter((h) =>
          (h.cleaned + " " + h.raw).toLowerCase().includes(historyFilter.toLowerCase()),
        ),
  );

  async function refreshHistory() {
    history = (await call<HistoryEntry[]>("history_list", { limit: 200 })) ?? history;
  }

  async function purgeHistory() {
    if (!confirm("Delete all history entries? This cannot be undone.")) return;
    const n = await call<number>("history_purge");
    if (n !== undefined) flash(`Purged ${n} entries`);
    await refreshHistory();
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      flash("Copied");
    } catch {
      flash("Copy failed", true);
    }
  }

  function fmtTime(ts: number): string {
    return new Date(ts).toLocaleString();
  }

  onMount(() => {
    loadAll();
    const perfPoll = setInterval(async () => {
      if (tab === "Performance") perf = (await call<Perf>("get_perf")) ?? perf;
      if (tab === "History") await refreshHistory();
    }, 1500);
    return () => clearInterval(perfPoll);
  });
</script>

<svelte:window onkeydown={onCaptureKey} />

<div class="shell">
  <nav>
    <div class="brand">OpenDictate</div>
    {#each tabs as t (t)}
      <button class="navitem" class:active={tab === t} onclick={() => (tab = t)}>{t}</button>
    {/each}
    <div class="spacer"></div>
    {#if status}<div class="status" class:error={statusIsError}>{status}</div>{/if}
  </nav>

  <main>
    {#if tab === "General" && settings}
      <h1>General</h1>

      <section>
        <h2>Profile</h2>
        <p class="hint">
          Controls how raw speech is cleaned up before insertion (email layout, code
          casing, meeting bullets, …).
        </p>
        <select
          value={settings.active_profile}
          onchange={(e) => onProfileSelect(e.currentTarget.value)}
        >
          {#each profiles as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
      </section>

      <section>
        <h2>Microphone</h2>
        <select
          value={settings.input_device ?? ""}
          onchange={async (e) => {
            if (!settings) return;
            settings.input_device = e.currentTarget.value === "" ? null : e.currentTarget.value;
            await saveSettings();
          }}
        >
          <option value="">System default</option>
          {#each devices as d (d.name)}
            <option value={d.name}>{d.name}{d.is_default ? " (default)" : ""}</option>
          {/each}
        </select>
        <p class="hint">Takes effect the next time dictation starts.</p>
      </section>

      <section>
        <h2>Hotkeys</h2>
        <div class="row">
          <span class="label">Toggle dictation</span>
          <button
            class="hotkey"
            class:capturing={capturing === "toggle"}
            onclick={() => (capturing = capturing === "toggle" ? null : "toggle")}
          >
            {capturing === "toggle" ? "Press keys… (Esc cancels)" : settings.hotkey_toggle}
          </button>
        </div>
        <div class="row">
          <span class="label">Push-to-talk (hold)</span>
          <button
            class="hotkey"
            class:capturing={capturing === "ptt"}
            onclick={() => (capturing = capturing === "ptt" ? null : "ptt")}
          >
            {capturing === "ptt" ? "Press keys… (Esc cancels)" : settings.hotkey_ptt}
          </button>
        </div>
        <p class="hint">Global shortcuts need at least one modifier (Ctrl/Shift/Alt).</p>
      </section>
    {:else if tab === "Cleanup" && profile}
      <h1>Cleanup — {profile.profile.name}</h1>
      <p class="hint">
        The 9-step cleanup chain that turns raw speech into insertable text. Changes are
        saved as your personal copy of this profile and apply from the next utterance.
      </p>

      <section>
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.whitespace.enabled} onchange={saveProfile} />
          Whitespace normalization
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.fillers.enabled} onchange={saveProfile} />
          Remove fillers (“um”, “uh”, “you know”, …)
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.dictionary.enabled} onchange={saveProfile} />
          Apply custom dictionary
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.symbols.enabled} onchange={saveProfile} />
          Spoken symbols (“at sign” → @) — table: {profile.cleanup.symbols.table}
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.punctuation.enabled} onchange={saveProfile} />
          Punctuation repair
        </label>
        {#if profile.cleanup.punctuation.enabled}
          <div class="sub">
            <label class="check">
              <input type="checkbox" bind:checked={profile.cleanup.punctuation.spoken} onchange={saveProfile} />
              Spoken punctuation (“comma”, “period” → , .)
            </label>
            <label class="check">
              <input type="checkbox" bind:checked={profile.cleanup.punctuation.ensure_terminal} onchange={saveProfile} />
              End sentences with a period
            </label>
          </div>
        {/if}
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.segmentation.enabled} onchange={saveProfile} />
          Sentence-boundary refinement
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.cleanup.capitalization.enabled} onchange={saveProfile} />
          Capitalization
        </label>
      </section>

      <section>
        <h2>Formatting</h2>
        <label class="check">
          <input type="checkbox" bind:checked={profile.format.casing_commands} onchange={saveProfile} />
          Casing commands (“camel case foo bar” → fooBar)
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.format.email_layout} onchange={saveProfile} />
          Email layout (breaks after greeting / around sign-off)
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={profile.format.bullets} onchange={saveProfile} />
          Bullet per utterance (meeting notes)
        </label>
      </section>
    {:else if tab === "Dictionary"}
      <h1>Dictionary</h1>
      <p class="hint">
        Personal vocabulary: what speech recognition tends to hear → what should be
        typed. Also biases recognition toward these words.
      </p>

      <div class="dictadd">
        <input placeholder="spoken form (e.g. open dictate)" bind:value={newSpoken} />
        <span class="arrow">→</span>
        <input placeholder="written form (e.g. OpenDictate)" bind:value={newWritten} />
        <label class="check small"><input type="checkbox" bind:checked={newCase} />exact case</label>
        <button class="primary" onclick={addEntry}>Add</button>
      </div>

      <table>
        <thead><tr><th>Spoken</th><th>Written</th><th></th><th></th></tr></thead>
        <tbody>
          {#each dict as e (e.spoken)}
            <tr>
              <td>{e.spoken}</td>
              <td>{e.written}</td>
              <td class="dim">{e.case_sensitive ? "exact case" : ""}</td>
              <td><button class="ghost" onclick={() => removeEntry(e.spoken)}>Remove</button></td>
            </tr>
          {:else}
            <tr><td colspan="4" class="dim">No entries yet.</td></tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === "History" && settings}
      <h1>History</h1>
      <p class="hint">
        Everything OpenDictate inserted, stored only on this device
        (%APPDATA%\OpenDictate). Recover text when insertion failed or you need it again.
      </p>

      <div class="row">
        <label class="check">
          <input
            type="checkbox"
            bind:checked={settings.history_enabled}
            onchange={saveSettings}
          />
          Keep history
        </label>
        <span class="label">Keep at most</span>
        <input
          class="num"
          type="number"
          min="10"
          max="10000"
          bind:value={settings.history_cap}
          onchange={saveSettings}
        />
        <span class="label">entries</span>
        <div class="spacer"></div>
        <button class="danger" onclick={purgeHistory}>Purge all</button>
      </div>

      <input class="filter" placeholder="Filter…" bind:value={historyFilter} />

      <table>
        <tbody>
          {#each historyShown as h (h.id)}
            <tr>
              <td class="dim time">{fmtTime(h.ts_ms)}<br /><span class="chip">{h.profile}</span></td>
              <td class="grow">{h.cleaned}</td>
              <td><button class="ghost" onclick={() => copyText(h.cleaned)}>Copy</button></td>
            </tr>
          {:else}
            <tr><td class="dim">No history{settings.history_enabled ? " yet" : " (disabled)"}.</td></tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === "Performance" && perf}
      <h1>Performance</h1>
      <p class="hint">Live pipeline latencies from this run. Targets from docs/02.</p>

      <table class="perf">
        <thead><tr><th>Metric</th><th>Current</th><th>Target</th></tr></thead>
        <tbody>
          <tr>
            <td>Cold start (process → hotkey live)</td>
            <td>{perf.cold_start_ms} ms</td>
            <td>≤ 2000 ms</td>
          </tr>
          <tr>
            <td>Speech end → final text (last)</td>
            <td>{perf.finalize_ms_last ?? "—"} {perf.finalize_ms_last != null ? "ms" : ""}</td>
            <td>≤ 300 ms</td>
          </tr>
          <tr>
            <td>Speech end → final text (best)</td>
            <td>{perf.finalize_ms_best ?? "—"} {perf.finalize_ms_best != null ? "ms" : ""}</td>
            <td></td>
          </tr>
          <tr>
            <td>Insertion (last)</td>
            <td>
              {perf.insert_ms_last ?? "—"} {perf.insert_ms_last != null ? "ms" : ""}
              {#if perf.insert_tier_last}<span class="chip">{perf.insert_tier_last}</span>{/if}
            </td>
            <td>≤ 30 ms</td>
          </tr>
          <tr><td>Utterances this run</td><td>{perf.utterances}</td><td></td></tr>
          <tr>
            <td>Insertions (ok / failed)</td>
            <td>{perf.inserts} / {perf.insert_failures}</td>
            <td>0 failed</td>
          </tr>
        </tbody>
      </table>
    {/if}
  </main>
</div>

<style>
  /* No :global(body) background here: all three windows share one CSS
     bundle, and a global paint would fill the transparent overlay window
     (main.ts sets the dark page background for non-overlay windows). */
  .shell {
    display: flex;
    min-height: 100vh;
    color: #e8e8ee;
    font:
      400 14px/1.5 "Segoe UI Variable",
      "Segoe UI",
      system-ui,
      sans-serif;
    user-select: none;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 176px;
    flex: none;
    padding: 16px 10px;
    background: #0f0f13;
    border-right: 1px solid rgba(255, 255, 255, 0.07);
    position: sticky;
    top: 0;
    height: 100vh;
    box-sizing: border-box;
  }
  .brand {
    font-weight: 600;
    font-size: 15px;
    padding: 6px 12px 14px;
    color: #fff;
  }
  .navitem {
    text-align: left;
    padding: 8px 12px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: #b9b9c6;
    font: inherit;
    cursor: pointer;
  }
  .navitem:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .navitem.active {
    background: rgba(61, 220, 132, 0.12);
    color: #3ddc84;
    font-weight: 600;
  }
  .spacer {
    flex: 1;
  }
  .status {
    font-size: 12px;
    color: #3ddc84;
    padding: 8px 12px;
  }
  .status.error {
    color: #ff6b6b;
    white-space: pre-wrap;
    user-select: text;
  }
  main {
    flex: 1;
    padding: 24px 32px 48px;
    max-width: 720px;
  }
  h1 {
    font-size: 20px;
    font-weight: 600;
    margin: 0 0 4px;
  }
  h2 {
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: #8a8a96;
    margin: 0 0 10px;
  }
  section {
    margin: 22px 0;
    padding: 16px;
    background: #1b1b22;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 12px;
  }
  .hint {
    color: #8a8a96;
    font-size: 12.5px;
    margin: 4px 0 10px;
  }
  select,
  input:not([type="checkbox"]) {
    background: #26262f;
    color: #e8e8ee;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 7px 10px;
    font: inherit;
    min-width: 240px;
    box-sizing: border-box;
  }
  input.num {
    min-width: 0;
    width: 90px;
  }
  input.filter {
    width: 100%;
    margin: 12px 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 8px 0;
    flex-wrap: wrap;
  }
  .label {
    color: #b9b9c6;
    min-width: 140px;
  }
  button {
    font: inherit;
    cursor: pointer;
  }
  .hotkey {
    background: #26262f;
    color: #e8e8ee;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    padding: 7px 14px;
    min-width: 220px;
    text-align: left;
    font-family: Consolas, monospace;
  }
  .hotkey.capturing {
    border-color: #3ddc84;
    color: #3ddc84;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 6px 0;
    cursor: pointer;
  }
  .check.small {
    font-size: 12.5px;
    color: #b9b9c6;
  }
  .check input {
    accent-color: #3ddc84;
    width: 15px;
    height: 15px;
  }
  .sub {
    margin-left: 24px;
    border-left: 2px solid rgba(255, 255, 255, 0.08);
    padding-left: 12px;
  }
  .primary {
    background: #3ddc84;
    color: #0d1117;
    border: 0;
    border-radius: 8px;
    padding: 8px 16px;
    font-weight: 600;
  }
  .danger {
    background: transparent;
    color: #ff6b6b;
    border: 1px solid rgba(255, 107, 107, 0.4);
    border-radius: 8px;
    padding: 7px 14px;
  }
  .ghost {
    background: transparent;
    color: #b9b9c6;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 6px;
    padding: 4px 10px;
    font-size: 12.5px;
  }
  .ghost:hover {
    color: #fff;
    border-color: rgba(255, 255, 255, 0.3);
  }
  .dictadd {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 14px 0;
    flex-wrap: wrap;
  }
  .dictadd input:not([type="checkbox"]) {
    min-width: 200px;
    flex: 1;
  }
  .arrow {
    color: #8a8a96;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 8px;
    user-select: text;
  }
  th {
    text-align: left;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: #8a8a96;
    padding: 6px 10px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }
  td {
    padding: 8px 10px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    vertical-align: top;
  }
  td.grow {
    width: 100%;
    white-space: pre-wrap;
    word-break: break-word;
  }
  td.time {
    white-space: nowrap;
    font-size: 12px;
  }
  .dim {
    color: #8a8a96;
  }
  .chip {
    display: inline-block;
    font-size: 11px;
    color: #3ddc84;
    background: rgba(61, 220, 132, 0.1);
    border-radius: 999px;
    padding: 1px 8px;
    margin-top: 2px;
  }
  .perf td:nth-child(2) {
    font-family: Consolas, monospace;
  }
</style>
