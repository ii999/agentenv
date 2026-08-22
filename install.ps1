#Requires -Version 7
<#
.SYNOPSIS
Installs an agentenv release binary from GitHub Releases on Windows,
together with the agentenv agent skill.

.DESCRIPTION
Downloads the x86_64 Windows archive for the requested release, verifies its
SHA-256 checksum, installs agentenv.exe into the install directory, and
installs the agent skill to ~\.agents\skills. Downloads use the GitHub CLI
when it is installed and signed in, which is required while the repository
is private; otherwise they use plain HTTPS.

.PARAMETER Version
Release tag to install, e.g. v0.1.1. Defaults to AGENTENV_VERSION or the
latest release.

.PARAMETER InstallDir
Binary install directory. Defaults to AGENTENV_INSTALL_DIR or
$env:LOCALAPPDATA\Programs\agentenv.

.PARAMETER ClaudeSkills
Also install the agent skill to ~\.claude\skills for Claude Code, in
addition to the ~\.agents\skills default.

.PARAMETER NoSkill
Install the binary only.
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

$useGh = $false
if (Get-Command gh -ErrorAction SilentlyContinue) {
    gh auth status *> $null
    if ($LASTEXITCODE -eq 0) { $useGh = $true }
}

if (-not $Version) {
    if ($useGh) {
        $Version = gh release view --repo $repo --json tagName --jq .tagName
        if ($LASTEXITCODE -ne 0 -or -not $Version) {
            throw 'Cannot determine the latest release through the GitHub CLI; pass -Version <tag>.'
        }
    }
    else {
        try {
            $Version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
        }
        catch {
            throw "Cannot determine the latest release; sign in with 'gh auth login' (required while the repository is private) or pass -Version <tag>."
        }
    }
}

$asset = "agentenv-$Version-$target.zip"
$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $workDir | Out-Null

function Get-ReleaseFile([string]$Name) {
    if ($script:useGh) {
        gh release download $script:Version --repo $script:repo --pattern $Name --dir $script:workDir
        if ($LASTEXITCODE -ne 0) {
            throw "Cannot download $Name from release $($script:Version) through the GitHub CLI."
        }
    }
    else {
        try {
            Invoke-WebRequest "https://github.com/$($script:repo)/releases/download/$($script:Version)/$Name" `
                -OutFile (Join-Path $script:workDir $Name)
        }
        catch {
            throw "Cannot download $Name from release $($script:Version); sign in with 'gh auth login' (required while the repository is private)."
        }
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

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item (Join-Path $extracted 'agentenv.exe') (Join-Path $InstallDir 'agentenv.exe') -Force
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
}
finally {
    Remove-Item $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
