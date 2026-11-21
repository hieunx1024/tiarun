# Quick Install Script for TiaRun on Windows (Portable)
$ErrorActionPreference = "Stop"

Write-Host "Installing TiaRun..." -ForegroundColor Cyan

$InstallDir = "$env:LOCALAPPDATA\Programs\tiarun"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$CurrentDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# Locate binary
$SourceExe = "$CurrentDir\tiarun.exe"
if (-not (Test-Path $SourceExe)) {
    $RepoExe = Join-Path $CurrentDir "..\..\target\release\tiarun.exe"
    if (Test-Path $RepoExe) {
        $SourceExe = (Resolve-Path $RepoExe).Path
    } else {
        throw "tiarun.exe not found at $SourceExe or $RepoExe. Please run 'cargo build --release' first."
    }
}

# Locate icon
$SourceIco = "$CurrentDir\tiarun.ico"
if (-not (Test-Path $SourceIco)) {
    $RepoIco = Join-Path $CurrentDir "..\..\assets\tiarun.ico"
    if (Test-Path $RepoIco) {
        $SourceIco = (Resolve-Path $RepoIco).Path
    }
}

# Stop running instance if any to avoid file lock
Get-Process -Name "tiarun" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 300

# Copy binary and icons
Copy-Item $SourceExe -Destination "$InstallDir\tiarun.exe" -Force
if (Test-Path $SourceIco) {
    Copy-Item $SourceIco -Destination "$InstallDir\tiarun.ico" -Force
}

# Add to User PATH
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    Write-Host "Added $InstallDir to User PATH." -ForegroundColor Green
}

$WshShell = New-Object -ComObject WScript.Shell

# 1. Create Startup Shortcut
$StartupDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Startup"
$Shortcut = $WshShell.CreateShortcut("$StartupDir\TiaRun.lnk")
$Shortcut.TargetPath = "$InstallDir\tiarun.exe"
$Shortcut.Arguments = ""
$Shortcut.WorkingDirectory = "$InstallDir"
if (Test-Path "$InstallDir\tiarun.ico") {
    $Shortcut.IconLocation = "$InstallDir\tiarun.ico, 0"
}
$Shortcut.Hotkey = "Ctrl+Alt+Space"
$Shortcut.Save()

# 2. Create Desktop Shortcut
$DesktopDir = [Environment]::GetFolderPath("Desktop")
$DeskShortcut = $WshShell.CreateShortcut("$DesktopDir\TiaRun.lnk")
$DeskShortcut.TargetPath = "$InstallDir\tiarun.exe"
$DeskShortcut.Arguments = ""
$DeskShortcut.WorkingDirectory = "$InstallDir"
if (Test-Path "$InstallDir\tiarun.ico") {
    $DeskShortcut.IconLocation = "$InstallDir\tiarun.ico, 0"
}
$DeskShortcut.Save()

# 3. Create Start Menu Shortcut
$StartMenuDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs"
$StartShortcut = $WshShell.CreateShortcut("$StartMenuDir\TiaRun.lnk")
$StartShortcut.TargetPath = "$InstallDir\tiarun.exe"
$StartShortcut.Arguments = ""
$StartShortcut.WorkingDirectory = "$InstallDir"
if (Test-Path "$InstallDir\tiarun.ico") {
    $StartShortcut.IconLocation = "$InstallDir\tiarun.ico, 0"
}
$StartShortcut.Save()

Write-Host "TiaRun installed successfully to $InstallDir!" -ForegroundColor Green
Write-Host "App is now ready! Press Ctrl + Alt + Space anywhere to launch." -ForegroundColor Yellow
