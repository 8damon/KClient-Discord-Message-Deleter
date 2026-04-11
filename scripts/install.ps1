$ErrorActionPreference = "Stop"

$repoRoot = Split-Path $PSScriptRoot -Parent
$binarySource = Join-Path $repoRoot "target\release\kclient.exe"
$installDir = Join-Path $HOME ".local\bin"
$installPath = Join-Path $installDir "kclient.exe"

Push-Location $repoRoot
try {
    cargo build --release --bin kclient
}
finally {
    Pop-Location
}

if (-not (Test-Path $binarySource)) {
    throw "Built binary not found at $binarySource"
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item -LiteralPath $binarySource -Destination $installPath -Force

Write-Host "Installed $installPath"
Write-Host "If '$installDir' is not on PATH yet, add it and open a new terminal."
