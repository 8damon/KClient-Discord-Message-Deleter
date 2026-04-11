$ErrorActionPreference = "Stop"

$repoRoot = Split-Path $PSScriptRoot -Parent
$paths = @()

if ($env:APPDATA) {
    $paths += Join-Path $env:APPDATA "kcordclient"
}

$paths += Join-Path $repoRoot "kcordclient"
if ($env:LOCALAPPDATA) {
    $paths += Join-Path $env:LOCALAPPDATA "kcordclient"
}

$installDir = Join-Path $HOME ".local\bin"
$binaries = @(
    Join-Path $installDir "kclient.exe",
    Join-Path $installDir "kclient.cmd",
    Join-Path $installDir "kcordclient.exe",
    Join-Path $installDir "keroklient.exe"
)

$cleanupCandidates = @(
    Join-Path $installDir "kclient.exe",
    Join-Path $repoRoot "target\release\kclient.exe"
)

foreach ($candidate in $cleanupCandidates) {
    if (Test-Path $candidate) {
        try {
            & $candidate --uninstall | Out-Host
            break
        }
        catch {
            Write-Host "Warning: uninstall cleanup via $candidate failed: $($_.Exception.Message)"
        }
    }
}

$removedAny = $false

foreach ($path in ($paths | Select-Object -Unique)) {
    if (Test-Path $path) {
        Remove-Item -LiteralPath $path -Recurse -Force
        Write-Host "Removed $path"
        $removedAny = $true
    }
}

foreach ($binary in $binaries) {
    if (Test-Path $binary) {
        Remove-Item -LiteralPath $binary -Force
        Write-Host "Removed $binary"
        $removedAny = $true
    }
}

if (-not $removedAny) {
    Write-Host "No local kcordclient data or installed binaries were found."
}
