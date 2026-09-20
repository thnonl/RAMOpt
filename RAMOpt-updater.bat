@echo off
setlocal EnableExtensions DisableDelayedExpansion

REM RAMOpt self-updater. It relocates itself before replacing installed files.
set "APP_EXE=RAMOpt.exe"
set "REPOSITORY=thnonl/RAMOpt"
set "INSTALL_DIR=%~dp0"
set "ASSET_NAME=RAMOpt-Windows-x64.zip"
set "SUMS_NAME=SHA256SUMS.txt"
set "PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not defined TEMP set "TEMP=%LOCALAPPDATA%\Temp"
if /I "%~1"=="/relocated" goto :relocated

set "RAMOPT_INSTALL_DIR=%~dp0"
if not exist "%PS%" (
    echo Windows PowerShell was not found at "%PS%".
    goto :fail
)
set "RAMOPT_UPDATER_DIR=%TEMP%\RAMOpt-update-%RANDOM%%RANDOM%%RANDOM%"
mkdir "%RAMOPT_UPDATER_DIR%" >nul 2>&1
if errorlevel 1 goto :fail
copy /y "%~f0" "%RAMOPT_UPDATER_DIR%\updater.bat" >nul 2>&1
if errorlevel 1 (
    rmdir /s /q "%RAMOPT_UPDATER_DIR%" >nul 2>&1
    goto :fail
)
start "" /b "%ComSpec%" /D /C ""%RAMOPT_UPDATER_DIR%\updater.bat" /relocated"
if errorlevel 1 goto :fail
exit /b 0

:relocated
set "INSTALL_DIR=%RAMOPT_INSTALL_DIR%"
set "TEMP_DIR=%RAMOPT_UPDATER_DIR%"
set "ARCHIVE=%TEMP_DIR%\%ASSET_NAME%"
set "SUMS=%TEMP_DIR%\%SUMS_NAME%"
set "EXTRACT_DIR=%TEMP_DIR%\files"
set "PID_FILE=%TEMP_DIR%\ramopt-pids.txt"

REM Snapshot only instances that existed before update started.
"%PS%" -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $target=Join-Path $env:RAMOPT_INSTALL_DIR $env:APP_EXE; @(Get-Process -ErrorAction SilentlyContinue | Where-Object { try { $_.Path -eq $target } catch { $false } }).Id | Set-Content -LiteralPath (Join-Path $env:RAMOPT_UPDATER_DIR 'ramopt-pids.txt')"
if errorlevel 1 goto :fail

"%PS%" -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $dir=$env:RAMOPT_UPDATER_DIR; $release=Invoke-RestMethod -Headers @{ 'User-Agent'='RAMOpt-Updater' } -Uri ('https://api.github.com/repos/' + $env:REPOSITORY + '/releases/latest') -TimeoutSec 60; $asset=$release.assets | Where-Object { $_.name -eq $env:ASSET_NAME } | Select-Object -First 1; $sum=$release.assets | Where-Object { $_.name -eq $env:SUMS_NAME } | Select-Object -First 1; if (-not $asset -or -not $sum) { throw 'Release archive or checksum manifest not found.' }; foreach ($item in @($asset,$sum)) { if ($item.browser_download_url -notmatch '^https://(github\.com|objects\.githubusercontent\.com)/') { throw 'Unexpected download host.' } }; Invoke-WebRequest -Headers @{ 'User-Agent'='RAMOpt-Updater' } -Uri $asset.browser_download_url -OutFile (Join-Path $dir $env:ASSET_NAME) -TimeoutSec 300; Invoke-WebRequest -Headers @{ 'User-Agent'='RAMOpt-Updater' } -Uri $sum.browser_download_url -OutFile (Join-Path $dir $env:SUMS_NAME) -TimeoutSec 60"
if errorlevel 1 goto :fail

"%PS%" -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $line=Get-Content -LiteralPath (Join-Path $env:RAMOPT_UPDATER_DIR $env:SUMS_NAME) | Where-Object { $_ -match [regex]::Escape($env:ASSET_NAME) } | Select-Object -First 1; if (-not $line) { throw 'Checksum entry missing; refusing to update.' }; $expected=(($line -split '\s+') | Where-Object { $_ -match '^[0-9A-Fa-f]{64}$' } | Select-Object -First 1); if (-not $expected) { throw 'Checksum entry malformed.' }; $actual=(Get-FileHash -LiteralPath (Join-Path $env:RAMOPT_UPDATER_DIR $env:ASSET_NAME) -Algorithm SHA256).Hash; if ($actual -ne $expected.ToUpperInvariant()) { throw 'Checksum mismatch; refusing to update.' }"
if errorlevel 1 goto :fail

"%PS%" -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $archive=Join-Path $env:RAMOPT_UPDATER_DIR $env:ASSET_NAME; if ((Get-Item -LiteralPath $archive).Length -gt 500MB) { throw 'Update archive is unexpectedly large.' }; Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $env:RAMOPT_UPDATER_DIR 'files') -Force"
if errorlevel 1 goto :fail

"%PS%" -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $target=Join-Path $env:RAMOPT_INSTALL_DIR $env:APP_EXE; $deadline=(Get-Date).AddMinutes(2); while ((Get-Process -ErrorAction SilentlyContinue | Where-Object { try { $_.Path -eq $target } catch { $false } }) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }; $ids=@(Get-Content -LiteralPath $env:PID_FILE -ErrorAction SilentlyContinue); foreach ($id in $ids) { $p=Get-Process -Id ([int]$id) -ErrorAction SilentlyContinue; if ($p) { try { if ($p.Path -eq $target) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } } catch {} } }; Start-Sleep -Milliseconds 500; if (Get-Process -ErrorAction SilentlyContinue | Where-Object { try { $_.Path -eq $target } catch { $false } }) { throw 'RAMOpt process did not exit; aborting update.' }"
if errorlevel 1 goto :fail

"%PS%" -NoProfile -NonInteractive -Command "$ErrorActionPreference='Stop'; $target=Join-Path $env:RAMOPT_INSTALL_DIR $env:APP_EXE; $source=Join-Path $env:RAMOPT_UPDATER_DIR 'files'; $backup=Join-Path $env:RAMOPT_UPDATER_DIR 'backup'; $payload=@($env:APP_EXE,'RAMOpt-updater.bat','LICENSE','README.md'); New-Item -ItemType Directory -Path $backup -Force | Out-Null; foreach ($name in $payload) { $old=Join-Path $env:RAMOPT_INSTALL_DIR $name; if (Test-Path -LiteralPath $old) { Copy-Item -LiteralPath $old -Destination (Join-Path $backup $name) -Force } }; try { foreach ($name in $payload) { $new=Join-Path $source $name; if (-not (Test-Path -LiteralPath $new)) { throw ('Package file missing: ' + $name) }; Copy-Item -LiteralPath $new -Destination (Join-Path $env:RAMOPT_INSTALL_DIR $name) -Force }; if (-not (Test-Path -LiteralPath $target)) { throw 'Installed executable missing after copy.' } } catch { foreach ($name in $payload) { $old=Join-Path $backup $name; if (Test-Path -LiteralPath $old) { Copy-Item -LiteralPath $old -Destination (Join-Path $env:RAMOPT_INSTALL_DIR $name) -Force } }; throw }; Start-Process -FilePath $target -WorkingDirectory $env:RAMOPT_INSTALL_DIR"
if errorlevel 1 goto :fail

rmdir /s /q "%TEMP_DIR%" >nul 2>&1
endlocal
exit /b 0

:fail
echo RAMOpt update failed. Restarting existing installation.
start "" /D "%INSTALL_DIR%" "%INSTALL_DIR%%APP_EXE%"
rmdir /s /q "%TEMP_DIR%" >nul 2>&1
endlocal
exit /b 5
