<#
.SYNOPSIS
    Prepares Windows to sideload the Limiara iOS app with xtool, then starts
    the guided setup inside WSL.

.DESCRIPTION
    1. Checks it runs as administrator, or starts itself again as
       administrator (Windows asks for permission).
    2. Installs WSL with Ubuntu 24.04 if it is missing. When Windows needs a
       restart, it says so: restart, then run this script again.
    3. Installs usbipd-win with winget if it is missing (it passes the
       iPhone's USB connection through to WSL).
    4. Inside Ubuntu, runs apps/device-shell/ios-xtool/cli/bootstrap.sh from
       a clone of the repository (cloning it first if needed). That installs
       git, curl and a C compiler with apt, rustup (verified), builds
       limiara-sideload and starts `limiara-sideload setup`.

    Nothing is downloaded and run from PowerShell: no Invoke-Expression of
    remote content. The execution policy is not changed (run it with
    `powershell -ExecutionPolicy Bypass -File setup-windows.ps1`, which
    applies to that one process only).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\setup-windows.ps1
#>
[CmdletBinding()]
param(
    [string]$Distro = 'Ubuntu-24.04',
    [string]$Branch = 'claude/xtool-sideload-spike',
    # The clone, relative to your Linux home directory.
    [string]$RepoDir = 'limiara-core'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RepoUrl = 'https://github.com/buchochelliq-labs/limiara-core'

function Write-Step([string]$Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

# Values passed into WSL's shell: plain names only.
foreach ($value in @($Distro, $Branch, $RepoDir)) {
    if ($value -notmatch '^[A-Za-z0-9._/-]+$' -or $value.StartsWith('-') -or $value.Contains('..')) {
        throw "Not a plain name: '$value'"
    }
}

# 1. Administrator rights (for WSL and usbipd-win).
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Step 'Administrator rights are needed to install WSL and usbipd-win; Windows will ask for permission.'
    $arguments = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"",
        '-Distro', $Distro, '-Branch', $Branch, '-RepoDir', $RepoDir
    )
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $arguments
    exit 0
}

# wsl.exe prints UTF-16 unless told otherwise.
$env:WSL_UTF8 = '1'

# 2. WSL with Ubuntu.
$installed = @()
if (Get-Command wsl.exe -ErrorAction SilentlyContinue) {
    $list = & wsl.exe --list --quiet 2>$null
    if ($LASTEXITCODE -eq 0 -and $list) {
        $installed = @($list | ForEach-Object { $_.Trim([char]0, ' ') } | Where-Object { $_ })
    }
}
if ($installed -notcontains $Distro) {
    Write-Step "Installing WSL with $Distro"
    & wsl.exe --install --distribution $Distro
    if ($LASTEXITCODE -ne 0) {
        throw "wsl --install failed (exit code $LASTEXITCODE)."
    }
    Write-Host ''
    Write-Host "If Windows asked you to restart, restart now. Then open '$Distro' from the Start menu once,"
    Write-Host 'create your Linux user name and password when it asks, and run this script again.'
    exit 0
}

$linuxUser = [string](& wsl.exe -d $Distro -- whoami)
if ($LASTEXITCODE -ne 0 -or -not $linuxUser.Trim() -or $linuxUser.Trim() -eq 'root') {
    Write-Host "Open '$Distro' from the Start menu once and create your Linux user, then run this script again."
    exit 1
}

# 3. usbipd-win.
if (-not (Get-Command usbipd -ErrorAction SilentlyContinue)) {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw 'winget is missing. Install "App Installer" from the Microsoft Store, or usbipd-win from https://github.com/dorssel/usbipd-win/releases, then run this script again.'
    }
    Write-Step 'Installing usbipd-win'
    & winget install --interactive --exact dorssel.usbipd-win
    if ($LASTEXITCODE -ne 0) {
        throw "winget could not install usbipd-win (exit code $LASTEXITCODE)."
    }
    # Pick up its PATH entry, and restart the distribution so WSL sees it.
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
        [Environment]::GetEnvironmentVariable('Path', 'User')
    & wsl.exe --terminate $Distro | Out-Null
}

# 4. Inside Ubuntu: git, a clone, then bootstrap.sh (build and start).
Write-Step "Preparing $Distro (sudo may ask for your Linux password)"
& wsl.exe -d $Distro --cd '~' -- bash -c 'command -v git >/dev/null || { sudo apt-get update && sudo apt-get install -y git ca-certificates; }'
if ($LASTEXITCODE -ne 0) { throw 'Installing git in WSL failed.' }

# No double quotes inside: Windows PowerShell mangles them on the way to
# native programs. The values were checked to be plain names above.
& wsl.exe -d $Distro --cd '~' -- bash -c 'test -e $1/.git || git clone --branch $2 -- $3 $1' _ $RepoDir $Branch $RepoUrl
if ($LASTEXITCODE -ne 0) { throw 'Cloning the repository in WSL failed.' }

Write-Step 'Building and starting limiara-sideload'
& wsl.exe -d $Distro --cd '~' -- bash "$RepoDir/apps/device-shell/ios-xtool/cli/bootstrap.sh" --branch $Branch
exit $LASTEXITCODE
