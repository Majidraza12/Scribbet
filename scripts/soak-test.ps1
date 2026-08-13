# M9 soak test: launch the app, sample working set + CPU while idle, and
# assert the resource targets (docs/01 success criteria):
#   RAM < 120 MB target / < 250 MB hard ceiling, CPU < 5% while idle.
#
# Usage:
#   powershell -File scripts\soak-test.ps1                 # 10 min (the M9 gate run)
#   powershell -File scripts\soak-test.ps1 -Minutes 1      # quick check
#   powershell -File scripts\soak-test.ps1 -ExePath path\to\scribbet-desktop.exe
#
# Run against the RELEASE binary for numbers that mean anything:
#   cargo build --release -p scribbet-desktop

param(
    [double]$Minutes = 10,
    [string]$ExePath = "target\release\scribbet-desktop.exe",
    [int]$SampleSeconds = 5
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $ExePath)) {
    throw "Binary not found: $ExePath (build with: cargo build --release -p scribbet-desktop)"
}

$proc = Start-Process -FilePath $ExePath -PassThru
Write-Host "Launched $ExePath (pid $($proc.Id)); settling 10 s..."
Start-Sleep -Seconds 10

$cores = [Environment]::ProcessorCount
$samples = [Math]::Max(1, [int]($Minutes * 60 / $SampleSeconds))
$wsMax = 0.0; $wsSum = 0.0; $cpuMax = 0.0; $cpuSum = 0.0

$prevCpu = (Get-Process -Id $proc.Id).TotalProcessorTime
for ($i = 1; $i -le $samples; $i++) {
    Start-Sleep -Seconds $SampleSeconds
    $p = Get-Process -Id $proc.Id -ErrorAction Stop
    $ws = $p.WorkingSet64 / 1MB
    $cpuNow = $p.TotalProcessorTime
    $cpuPct = ($cpuNow - $prevCpu).TotalSeconds / ($SampleSeconds * $cores) * 100
    $prevCpu = $cpuNow

    $wsSum += $ws; $cpuSum += $cpuPct
    if ($ws -gt $wsMax) { $wsMax = $ws }
    if ($cpuPct -gt $cpuMax) { $cpuMax = $cpuPct }
    Write-Host ("[{0,4}/{1}] WS {2,7:N1} MB   CPU {3,5:N2} %" -f $i, $samples, $ws, $cpuPct)
}

Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue

$wsAvg = $wsSum / $samples; $cpuAvg = $cpuSum / $samples
Write-Host ""
Write-Host ("RESULT  WS avg {0:N1} MB / max {1:N1} MB   CPU avg {2:N2} % / max {3:N2} %" -f $wsAvg, $wsMax, $cpuAvg, $cpuMax)

$fail = $false
if ($wsMax -ge 250) { Write-Host "FAIL: working set exceeded the 250 MB hard ceiling"; $fail = $true }
elseif ($wsMax -ge 120) { Write-Host "WARN: working set over the 120 MB target (hard ceiling is 250 MB)" }
if ($cpuAvg -ge 5) { Write-Host "FAIL: average idle CPU >= 5%"; $fail = $true }

if ($fail) { exit 1 }
Write-Host "Soak targets met."
