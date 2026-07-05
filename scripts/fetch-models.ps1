# Dev-time model fetcher (M2). The user-facing download flow with pinned
# checksums ships in M8; until then this script is how a dev machine gets the
# STT model. Deliberately a script, not library code: crates stay free of
# network code paths (docs/06-security.md, ADR-7).
#
# Silero VAD is NOT fetched here: it is small (2.3 MB), MIT-licensed, and
# committed at crates/vad/models/silero_vad.onnx (embedded into the binary).
#
# Usage:  powershell -File scripts\fetch-models.ps1

$ErrorActionPreference = "Stop"

$modelDir = Join-Path $env:LOCALAPPDATA "OpenDictate\models"
New-Item -ItemType Directory -Force $modelDir | Out-Null

$models = @(
    @{
        Name = "ggml-base.en-q5_1.bin"
        Url  = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin"
        # SHA-256 computed from the 2026-07-04 fetch of this file.
        # M8 ships pinned checksums in the app.
        Sha256 = "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f"
    }
)

foreach ($m in $models) {
    $dest = Join-Path $modelDir $m.Name
    if (Test-Path $dest) {
        $hash = (Get-FileHash $dest -Algorithm SHA256).Hash.ToLower()
        if ($hash -eq $m.Sha256) {
            Write-Host "OK (cached): $($m.Name)"
            continue
        }
        Write-Host "Checksum mismatch on cached $($m.Name); re-downloading."
        Remove-Item $dest -Force
    }
    Write-Host "Downloading $($m.Name)..."
    Invoke-WebRequest -Uri $m.Url -OutFile $dest
    $hash = (Get-FileHash $dest -Algorithm SHA256).Hash.ToLower()
    if ($m.Sha256 -and $hash -ne $m.Sha256) {
        Remove-Item $dest -Force
        throw "Checksum mismatch for $($m.Name): got $hash, expected $($m.Sha256)"
    }
    Write-Host "OK: $($m.Name) sha256=$hash"
}

Write-Host "Models in $modelDir"
