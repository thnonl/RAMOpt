@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "INSTALL_DIR=%~dp0"
set "APP_EXE=RAMOpt.exe"
set "REPOSITORY=thnonl/RAMOpt"
set "ASSET_NAME=RAMOpt-Windows-x64.zip"
set "TEMP_DIR=%TEMP%\RAMOpt-update-%RANDOM%%RANDOM%"
set "ARCHIVE=%TEMP_DIR%\%ASSET_NAME%"
set "EXTRACT_DIR=%TEMP_DIR%\files"

mkdir "%TEMP_DIR%" >nul 2>&1 || exit /b 3

powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; $release = Invoke-RestMethod -Headers @{ 'User-Agent' = 'RAMOpt-Updater' } -Uri 'https://api.github.com/repos/%REPOSITORY%/releases/latest'; $asset = $release.assets | Where-Object { $_.name -eq '%ASSET_NAME%' } | Select-Object -First 1; if (-not $asset) { throw 'Release archive not found.' }; Invoke-WebRequest -Headers @{ 'User-Agent' = 'RAMOpt-Updater' } -Uri $asset.browser_download_url -OutFile '%ARCHIVE%'"
if errorlevel 1 goto :cleanup

powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; Expand-Archive -LiteralPath '%ARCHIVE%' -DestinationPath '%EXTRACT_DIR%' -Force"
if errorlevel 1 goto :cleanup

powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$ErrorActionPreference = 'Stop'; $target = Join-Path '%INSTALL_DIR%' '%APP_EXE%'; $deadline = (Get-Date).AddMinutes(2); while ((Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $target }) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }; if (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $target }) { Stop-Process -Force -ErrorAction Stop -InputObject (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $target }) }; Get-ChildItem -LiteralPath '%EXTRACT_DIR%' -Force | Copy-Item -Destination '%INSTALL_DIR%' -Recurse -Force; Start-Process -FilePath (Join-Path '%INSTALL_DIR%' '%APP_EXE%')"

:cleanup
rmdir /s /q "%TEMP_DIR%" >nul 2>&1
endlocal
