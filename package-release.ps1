$ErrorActionPreference = 'Stop'

$projectRoot = $PSScriptRoot
$packageDir = Join-Path $projectRoot 'release\RAMOpt'
$binary = Join-Path $projectRoot 'target\release\ramopt.exe'

Push-Location $projectRoot
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "Release build failed with exit code $LASTEXITCODE."
    }

    # Preserve settings.json (and any other user files) across runs.
    New-Item -ItemType Directory -Path $packageDir -Force | Out-Null

    foreach ($f in 'RAMOpt.exe', 'RAMOpt-updater.bat', 'LICENSE', 'README.md') {
        Remove-Item -Force (Join-Path $packageDir $f) -ErrorAction SilentlyContinue
    }

    Copy-Item $binary (Join-Path $packageDir 'RAMOpt.exe')
    Copy-Item (Join-Path $projectRoot 'RAMOpt-updater.bat') $packageDir
    Copy-Item (Join-Path $projectRoot 'LICENSE') $packageDir
    Copy-Item (Join-Path $projectRoot 'README.md') $packageDir

    Write-Host "GitHub Release package created: $packageDir"
} finally {
    Pop-Location
}
