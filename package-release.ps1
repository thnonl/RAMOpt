$ErrorActionPreference = 'Stop'

$projectRoot = $PSScriptRoot
$packageDir = Join-Path $projectRoot 'release\RAMOpt'
$binary = Join-Path $projectRoot 'target\release\ramopt.exe'

Push-Location $projectRoot
try {
    $previousManifestFlag = [Environment]::GetEnvironmentVariable('RAMOPT_EMBED_ADMIN_MANIFEST', 'Process')
    $env:RAMOPT_EMBED_ADMIN_MANIFEST = '1'
    try {
        cargo build --release --locked
        $buildExitCode = $LASTEXITCODE
    } finally {
        if ($null -eq $previousManifestFlag) {
            Remove-Item Env:RAMOPT_EMBED_ADMIN_MANIFEST -ErrorAction SilentlyContinue
        } else {
            $env:RAMOPT_EMBED_ADMIN_MANIFEST = $previousManifestFlag
        }
    }
    if ($buildExitCode -ne 0) {
        throw "Release build failed with exit code $LASTEXITCODE."
    }

    # Build the package from a clean staging directory so stale user files
    # (settings.json, ramopt.log, previous builds) never ship in the archive.
    if (Test-Path $packageDir) {
        Remove-Item -Recurse -Force $packageDir
    }
    New-Item -ItemType Directory -Path $packageDir -Force | Out-Null

    $files = 'RAMOpt.exe', 'RAMOpt-updater.bat', 'LICENSE', 'README.md'
    $copied = @()

    Copy-Item $binary (Join-Path $packageDir 'RAMOpt.exe')
    Copy-Item (Join-Path $projectRoot 'RAMOpt-updater.bat') $packageDir
    Copy-Item (Join-Path $projectRoot 'LICENSE') $packageDir
    Copy-Item (Join-Path $projectRoot 'README.md') $packageDir

    foreach ($file in $files) {
        $path = Join-Path $packageDir $file
        if (-not (Test-Path $path)) {
            throw "Release package is missing $file"
        }
        if ((Get-Item $path).Length -eq 0) {
            throw "Release package file $file is empty"
        }
        $copied += (Get-Item $path).Name
    }

    $extra = Get-ChildItem -LiteralPath $packageDir -File | Where-Object { $_.Name -notin $copied }
    if ($extra) {
        throw "Release package contains unexpected files: $($extra.Name -join ', ')"
    }

    # Emit a checksum manifest next to the package so the updater can verify
    # the downloaded archive before extracting it.
    $zipPath = Join-Path $projectRoot 'release\RAMOpt-Windows-x64.zip'
    if (Test-Path $zipPath) {
        Remove-Item -Force $zipPath
    }
    Compress-Archive -Path (Join-Path $packageDir '*') -DestinationPath $zipPath -Force

    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToUpperInvariant()
    $sumsPath = Join-Path $projectRoot 'release\SHA256SUMS.txt'
    "$hash  RAMOpt-Windows-x64.zip" | Set-Content -LiteralPath $sumsPath -Encoding ascii

    Write-Host "GitHub Release package created: $packageDir"
    Write-Host "Archive: $zipPath"
    Write-Host "Checksum manifest: $sumsPath"
} finally {
    Pop-Location
}

exit 0
