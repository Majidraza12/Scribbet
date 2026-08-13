# Scribbet — Machine Bootstrap

Playbook for setting up Scribbet on a fresh Windows machine from a clone of
this repo. Written to be executed by Claude Code, but every step works by hand.

**Paste this prompt into Claude Code after cloning:**

> Read BOOTSTRAP.md and set this machine up: install whatever toolchain is
> missing, build the installer (GPU build if this machine has a Vulkan-capable
> GPU, CPU build otherwise), install it, launch it, and verify the overlay
> pill appears bottom-center. Tell me when I can dictate.

After the VS C++ workload lands, the compiler is only on PATH inside a
**Developer PowerShell for VS 2022** (or after running `vcvars64.bat`). If a
build fails with "cl not found" or a missing `cmake`, reopen the shell as
Developer PowerShell and retry from step 3.

---

## 1. Toolchain (install only what's missing)

Check each; install via winget where absent:

| Tool | Check | Install |
|---|---|---|
| VS Build Tools + C++ | `Get-Command cl` after vcvars, or look for `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools` | see command below — winget alone does NOT install the C++ workload |
| Rust (MSVC) | `cargo --version` | `winget install Rustlang.Rustup` then `rustup default stable-msvc` |
| Node.js LTS | `node --version` | `winget install OpenJS.NodeJS.LTS` |
| CMake | `cmake --version` | Ships inside VS Build Tools (`...\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`) — add to PATH, or `winget install Kitware.CMake` |
| LLVM (libclang, for whisper bindgen) | `Test-Path "C:\Program Files\LLVM\bin\libclang.dll"` | `winget install LLVM.LLVM` |
| Vulkan SDK (GPU build only) | `Test-Path $env:VULKAN_SDK` | `winget install KhronosGroup.VulkanSDK` |

**VS Build Tools — the one with a trap.** Installing the bare package gives you
no compiler; the C++ workload must be named explicitly. Install (or repair an
existing bare install) with:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools `
  --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

`--includeRecommended` pulls in the Windows SDK and CMake that ship inside the
workload. After this, `cl` resolves inside a Developer PowerShell (or after
running `vcvars64.bat`). If `cl` still isn't found, the workload didn't take —
re-run the line above; do not proceed to `cargo build` without it (whisper's
C++ will fail to compile).

## 2. Environment quirks (all hard-won; skip none)

```powershell
# libclang for whisper-rs bindgen
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"

# SHORT target dir. Non-negotiable for GPU builds: MSVC FileTracker dies
# with FTK1011 (MAX_PATH) inside vulkan-shaders-gen under a long repo path.
$env:CARGO_TARGET_DIR = "C:\odt"

# GPU build only (adjust version to what installed):
$env:VULKAN_SDK = "C:\VulkanSDK\<version>"
$env:Path = "$env:VULKAN_SDK\Bin;" + $env:Path

# Fresh shells sometimes miss cargo:
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
```

## 3. Build

```powershell
cd apps\desktop
npm install

# GPU (any Vulkan-capable GPU — discrete or iGPU; runtime falls back to CPU):
npx tauri build -- --features gpu-vulkan

# CPU-only machine:
npx tauri build
```

Bundles land in `C:\odt\release\bundle\` (`nsis\` and `msi\`).

## 4. Install and run

```powershell
& "C:\odt\release\bundle\nsis\Scribbet_<version>_x64-setup.exe" /S
& "$env:LOCALAPPDATA\Scribbet\scribbet-desktop.exe"
```

- Installs per-user to `%LOCALAPPDATA%\Scribbet`. Never run the exe from
  the build dir — the file lock breaks the next rebuild.
- Unsigned binary: SmartScreen shows "unknown publisher" on interactive
  install. More info → Run anyway. (`/S` silent install skips the dialog.)
- First launch opens onboarding, which downloads the STT model
  (checksum-pinned; this is the app's only network access).

## 5. Verify

1. Tiny translucent pill sits bottom-center, just above the taskbar.
2. Open Notepad, press `Ctrl+Shift+Space`, talk, press it again — text
   inserts on stop. `Ctrl+Shift+D` is push-to-talk. Clicking the pill also
   toggles.
3. If another dictation app runs (e.g. Wispr Flow), quit it first — hotkey
   and mic contention causes ghost failures.

## 6. Optional: bigger STT model

Default is `base.en` (fast, light). For noticeably better accuracy on a
machine with GPU headroom, drop `ggml-large-v3-turbo-q5_0.bin` into
`%LOCALAPPDATA%\Scribbet\models\` and set `"stt_model"` to that file name
in `%APPDATA%\Scribbet\settings.json` (Settings window can do this too).
Missing/typo'd model name falls back to base.en — the app never starts deaf.

**Warning:** `settings.json` is BOM-sensitive. If editing from PowerShell 5.1,
do NOT use `Out-File -Encoding utf8` (writes a BOM; app rejects the file and
reverts to defaults). Use `[IO.File]::WriteAllText($path, $json,
[Text.UTF8Encoding]::new($false))` or edit in the Settings window.
