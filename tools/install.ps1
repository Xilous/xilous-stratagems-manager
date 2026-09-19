#Requires -Version 5.1
<#
.SYNOPSIS
    Installs the release build for the current user and creates shortcuts.

.DESCRIPTION
    Copies target\release\XilousStratagemsManager.exe (and LICENSE) to
    %LOCALAPPDATA%\Programs\Xilous Stratagems Manager, creates a Start Menu
    shortcut, and optionally desktop and startup shortcuts. Runtime data
    (config, presets, captured icons, active loadout) lives in the install
    folder under data\ and is preserved across reinstalls.

.PARAMETER MigrateFrom
    A data folder to copy into the install folder when it has no data yet
    (e.g. target\release\data after running from the build folder).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools/install.ps1 -Launch
    powershell -ExecutionPolicy Bypass -File tools/install.ps1 -Desktop -Startup
#>
[CmdletBinding()]
param(
    [string]$Root = "",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Xilous Stratagems Manager'),
    [string]$MigrateFrom = "",
    [switch]$Desktop,
    [switch]$Startup,
    [switch]$Launch
)

$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path) }
$ExeName = 'XilousStratagemsManager.exe'
$AppName = 'Xilous Stratagems Manager'
$Source = Join-Path $Root "target\release\$ExeName"
if (-not (Test-Path $Source)) {
    throw "Release build not found at $Source. Run: cargo build --release"
}

# Stop a running instance so the executable can be replaced.
Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($ExeName)) -ErrorAction SilentlyContinue |
    ForEach-Object {
        Write-Host "Stopping running instance (PID $($_.Id))"
        $_ | Stop-Process -Force
    }
Start-Sleep -Milliseconds 400

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $Source -Destination (Join-Path $InstallDir $ExeName) -Force
Copy-Item -Path (Join-Path $Root 'LICENSE') -Destination $InstallDir -Force
Write-Host "Installed $ExeName to $InstallDir"

# Bring existing data along once, never overwriting data already in place.
$DataDir = Join-Path $InstallDir 'data'
if ($MigrateFrom -and (Test-Path $MigrateFrom) -and -not (Test-Path (Join-Path $DataDir 'presets.json'))) {
    New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
    foreach ($item in 'config.toml', 'presets.json', 'active_loadout.json', 'local_templates') {
        $from = Join-Path $MigrateFrom $item
        if (Test-Path $from) {
            Copy-Item -Path $from -Destination $DataDir -Recurse -Force
            Write-Host "  migrated $item"
        }
    }
}

function New-Shortcut([string]$Path) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $shortcut.TargetPath = Join-Path $InstallDir $ExeName
    $shortcut.WorkingDirectory = $InstallDir
    $shortcut.IconLocation = (Join-Path $InstallDir $ExeName) + ',0'
    $shortcut.Description = 'Helldivers 2 loadout presets and stratagem hotkeys'
    $shortcut.Save()
    Write-Host "Shortcut: $Path"
}

$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
New-Shortcut (Join-Path $StartMenu "$AppName.lnk")
if ($Desktop) {
    New-Shortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) "$AppName.lnk")
}
$StartupLink = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\$AppName.lnk"
if ($Startup) {
    New-Shortcut $StartupLink
} elseif (Test-Path $StartupLink) {
    Write-Host "Startup shortcut kept: $StartupLink"
}

if ($Launch) {
    Start-Process -FilePath (Join-Path $InstallDir $ExeName) -WorkingDirectory $InstallDir
    Write-Host "Launched $AppName"
}
