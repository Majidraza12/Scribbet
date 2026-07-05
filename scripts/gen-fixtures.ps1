# Generates speech fixture WAVs (16 kHz mono s16) via Windows SAPI TTS.
# Committed under testdata/ so tests are reproducible; re-run only to add
# fixtures (TTS voices differ per machine - regenerating changes waveforms).
#
# Usage:  powershell -File scripts\gen-fixtures.ps1

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Speech

$outDir = Join-Path $PSScriptRoot "..\testdata\speech"
New-Item -ItemType Directory -Force $outDir | Out-Null

$fixtures = @(
    @{ File = "hello_world.wav";    Text = "Hello world. This is a test of the dictation system." }
    @{ File = "quick_fox.wav";      Text = "The quick brown fox jumps over the lazy dog." }
    @{ File = "two_sentences.wav";  Text = "The meeting starts at nine. Please bring the quarterly report." }
    @{ File = "with_pause.wav";     Text = "First part of the utterance." ; Text2 = "Second part after a pause." }
)

$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(
    16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
    [System.Speech.AudioFormat.AudioChannel]::Mono)

foreach ($f in $fixtures) {
    $path = Join-Path $outDir $f.File
    $synth.SetOutputToWaveFile($path, $format)
    $synth.Speak($f.Text)
    if ($f.Text2) {
        # ~900 ms of silence between utterances, for VAD boundary tests.
        $builder = New-Object System.Speech.Synthesis.PromptBuilder
        $builder.AppendBreak([TimeSpan]::FromMilliseconds(900))
        $builder.AppendText($f.Text2)
        $synth.Speak($builder)
    }
    $synth.SetOutputToNull()
    Write-Host "wrote $path"
}
$synth.Dispose()
Write-Host "done"
