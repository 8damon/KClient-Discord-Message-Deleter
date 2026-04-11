$ErrorActionPreference = "Stop"

$appData = if ($env:APPDATA) { Join-Path $env:APPDATA "kcordclient" } else { Join-Path (Get-Location) "kcordclient" }

if (Test-Path $appData) {
    Remove-Item -LiteralPath $appData -Recurse -Force
    Write-Host "Removed $appData"
} else {
    Write-Host "No local kcordclient data found at $appData"
}

Write-Host "Delete kcordclient.exe / kclient.exe from their install location if you no longer want the binaries."
