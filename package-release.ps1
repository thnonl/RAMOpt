$ErrorActionPreference = 'Stop'

$projectRoot = $PSScriptRoot
$packageDir = Join-Path $projectRoot 'release\RAMOpt'
$binary = Join-Path $projectRoot 'target\release\ramopt.exe'

Push-Location $projectRoot
try {
    cargo build --release

    Remove-Item -Recurse -Force $packageDir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $packageDir -Force | Out-Null

    Copy-Item $binary (Join-Path $packageDir 'RAMOpt.exe')
    Copy-Item (Join-Path $projectRoot 'LICENSE') $packageDir
    Copy-Item (Join-Path $projectRoot 'README.md') $packageDir

    Write-Host "GitHub Release package created: $packageDir"
} finally {
    Pop-Location
}
