#Requires -Version 7
<#
.SYNOPSIS
Installs an agentenv release binary from GitHub Releases on Windows,
together with the agentenv agent skill.

.DESCRIPTION
Downloads the x86_64 Windows archive for the requested release, verifies its
SHA-256 checksum, installs the matching executable bundle, and
installs the agent skill to ~\.agents\skills. Downloads use plain HTTPS
from GitHub Releases.

.PARAMETER Version
Release tag to install, e.g. v0.3.0. Defaults to AGENTENV_VERSION or the
latest release.

.PARAMETER InstallDir
Binary install directory. Defaults to AGENTENV_INSTALL_DIR or
$env:LOCALAPPDATA\Programs\agentenv.

.PARAMETER ClaudeSkills
Also install the agent skill to ~\.claude\skills for Claude Code, in
addition to the ~\.agents\skills default.

.PARAMETER NoSkill
Install the executable bundle only.
#>
param(
    [string]$Version = $env:AGENTENV_VERSION,
    [string]$InstallDir = $(if ($env:AGENTENV_INSTALL_DIR) { $env:AGENTENV_INSTALL_DIR }
        else { Join-Path $env:LOCALAPPDATA 'Programs\agentenv' }),
    [switch]$ClaudeSkills,
    [switch]$NoSkill
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repo = 'ii999/agentenv'
$target = 'x86_64-pc-windows-msvc'

if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw "No prebuilt binary for Windows $env:PROCESSOR_ARCHITECTURE; build from source with 'cargo build --release'."
}

if (-not $Version) {
    try {
        $Version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
    }
    catch {
        throw 'Cannot determine the latest release; pass -Version <tag>.'
    }
}

$asset = "agentenv-$Version-$target.zip"
$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $workDir | Out-Null

function Get-ReleaseFile([string]$Name) {
    try {
        Invoke-WebRequest "https://github.com/$($script:repo)/releases/download/$($script:Version)/$Name" `
            -OutFile (Join-Path $script:workDir $Name)
    }
    catch {
        throw "Cannot download $Name from release $($script:Version)."
    }
}

function Install-SkillTo([string]$Root) {
    $destination = Join-Path $Root 'agentenv'
    if ((Test-Path $destination) -and -not (Test-Path (Join-Path $destination 'SKILL.md'))) {
        throw "$destination exists but is not an agentenv skill directory; move it aside and rerun."
    }
    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    if (Test-Path $destination) { Remove-Item $destination -Recurse -Force }
    Copy-Item $script:packagedSkill $destination -Recurse
    Write-Host "Installed the agentenv agent skill to $destination"
}

try {
    Write-Host "Downloading agentenv $Version for $target..."
    Get-ReleaseFile $asset
    Get-ReleaseFile 'SHA256SUMS'

    $sumLine = Get-Content (Join-Path $workDir 'SHA256SUMS') | Where-Object { $_ -match [regex]::Escape($asset) }
    if (-not $sumLine) { throw "SHA256SUMS carries no entry for $asset." }
    $expected = ($sumLine -split '\s+')[0]
    $actual = (Get-FileHash (Join-Path $workDir $asset) -Algorithm SHA256).Hash
    if ($actual -ne $expected) { throw "Checksum verification failed for $asset." }

    Expand-Archive (Join-Path $workDir $asset) -DestinationPath $workDir
    $extracted = Join-Path $workDir "agentenv-$Version-$target"
    $releaseVersion = $Version.TrimStart('v')
    $binaries = @('agentenv.exe', 'agentenv-sudo-helper.exe', 'agentenv-ssh-askpass.exe')
    foreach ($binary in $binaries) {
        if (-not (Test-Path (Join-Path $extracted $binary) -PathType Leaf)) {
            throw "$asset is incomplete: missing $binary; install a complete release bundle."
        }
    }
    $mainIdentity = & (Join-Path $extracted 'agentenv.exe') --version
    if ($LASTEXITCODE -ne 0 -or $mainIdentity -ne "agentenv $releaseVersion") {
        throw "$asset contains a mismatched agentenv executable."
    }
    $sudoIdentity = & (Join-Path $extracted 'agentenv-sudo-helper.exe') --identity
    if ($LASTEXITCODE -ne 0 -or $sudoIdentity -ne "agentenv-sudo-helper 1 $releaseVersion") {
        throw "$asset contains a mismatched sudo helper."
    }
    $sshIdentity = & (Join-Path $extracted 'agentenv-ssh-askpass.exe') --identity
    if ($LASTEXITCODE -ne 0 -or $sshIdentity -ne "agentenv-ssh-askpass 1 $releaseVersion") {
        throw "$asset contains a mismatched SSH askpass helper."
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    foreach ($binary in $binaries) {
        $staged = Join-Path $InstallDir ".$binary.agentenv-new"
        $backup = Join-Path $InstallDir ".$binary.agentenv-old"
        Remove-Item $staged, $backup -Force -ErrorAction SilentlyContinue
        Copy-Item (Join-Path $extracted $binary) $staged
    }
    $swapped = [System.Collections.Generic.List[string]]::new()
    try {
        foreach ($binary in @('agentenv-sudo-helper.exe', 'agentenv-ssh-askpass.exe', 'agentenv.exe')) {
            $destination = Join-Path $InstallDir $binary
            $staged = Join-Path $InstallDir ".$binary.agentenv-new"
            $backup = Join-Path $InstallDir ".$binary.agentenv-old"
            if (Test-Path $destination) { Move-Item $destination $backup }
            $swapped.Add($binary)
            Move-Item $staged $destination
        }
    }
    catch {
        $installFailure = $_.Exception.Message
        $rollbackFailure = $null
        for ($index = $swapped.Count - 1; $index -ge 0; $index--) {
            $binary = $swapped[$index]
            $destination = Join-Path $InstallDir $binary
            $backup = Join-Path $InstallDir ".$binary.agentenv-old"
            try {
                if (Test-Path $destination) {
                    Remove-Item $destination -Force -ErrorAction Stop
                }
                if (Test-Path $destination) {
                    throw "Rollback could not remove $destination."
                }
                if (Test-Path $backup) { Move-Item $backup $destination }
            }
            catch {
                if (-not $rollbackFailure) { $rollbackFailure = $_.Exception.Message }
            }
        }
        foreach ($binary in $binaries) {
            Remove-Item (Join-Path $InstallDir ".$binary.agentenv-new") -Force -ErrorAction SilentlyContinue
        }
        if ($rollbackFailure) {
            throw "Cannot install or restore the complete executable bundle; rerun the installer to repair it. Install error: $installFailure Rollback error: $rollbackFailure"
        }
        throw "Cannot install the complete executable bundle; the previous bundle was restored. $installFailure"
    }
    foreach ($binary in $binaries) {
        Remove-Item (Join-Path $InstallDir ".$binary.agentenv-old") -Force -ErrorAction SilentlyContinue
    }
    $installed = & (Join-Path $InstallDir 'agentenv.exe') --version
    Write-Host "Installed $installed to $(Join-Path $InstallDir 'agentenv.exe')"

    if (-not $NoSkill) {
        $packagedSkill = Join-Path $extracted 'skills\agentenv'
        if (Test-Path (Join-Path $packagedSkill 'SKILL.md')) {
            Install-SkillTo (Join-Path $HOME '.agents\skills')
            if ($ClaudeSkills) {
                Install-SkillTo (Join-Path $HOME '.claude\skills')
            }
        }
        else {
            Write-Warning "Release $Version ships no agent skill; skipping the skill install."
        }
    }

    $onPath = ($env:Path -split ';') -contains $InstallDir
    if (-not $onPath) {
        Write-Host "Add $InstallDir to PATH to run 'agentenv' from any directory."
    }
    Write-Host "Later releases install with 'agentenv update'."
}
finally {
    Remove-Item $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
