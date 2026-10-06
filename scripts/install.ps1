#Requires -Version 5.1
<#
.SYNOPSIS
    Installs dsh-supercollider into the DeepSeek Harness web profile on this
    machine, straight from this repository.

.DESCRIPTION
    The vncode pack installs this package with `scripts/install.bat` in that
    repository. This is the standalone path: it registers THIS checkout's
    `plugin/` folder with the harness, so a change made here is live after a
    restart with no re-install.

    It runs on Windows PowerShell 5.1 and on PowerShell 7+. macOS and Linux run
    scripts/install.sh, which does the same work in POSIX shell.

    One target only: the raw CLI/web install used by `npx @deepseek-ai/dsh web`
    ($DSH_HOME, else ~/.dsh, profile "web" by default).

.PARAMETER PluginPath
    The package folder to install. Default: this repository's plugin/.

.PARAMETER ProfileName
    The harness profile to install into. Default: web, or $DSH_PROFILE.

.PARAMETER DshHome
    Override $DSH_HOME. Default: $env:DSH_HOME, else ~/.dsh.

.PARAMETER Force
    Re-add even when the profile already lists the same version.

.PARAMETER Uninstall
    Remove the package from the profile instead.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/install.ps1
.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/install.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [string]$PluginPath = '',
    [string]$ProfileName = '',
    [string]$DshHome = '',
    [switch]$Force,
    [switch]$Uninstall,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Text) Write-Host ''; Write-Host "== $Text" -ForegroundColor Cyan }
function Write-Note { param([string]$Text) Write-Host "  $Text" }

function Show-Usage {
    Write-Host ''
    Write-Host 'Usage: scripts\install.ps1 [flags]'
    Write-Host ''
    Write-Host '  -PluginPath <dir>   the package folder to install (default: this repo''s plugin\)'
    Write-Host '  -ProfileName <n>    harness profile (default: web, else $DSH_PROFILE)'
    Write-Host '  -DshHome <dir>      harness home (default: $DSH_HOME, else ~/.dsh)'
    Write-Host '  -Force              re-add even when the version is unchanged'
    Write-Host '  -Uninstall          remove the package instead of adding it'
    Write-Host ''
}

if ($Help) { Show-Usage; exit 0 }

$repoRoot = Split-Path -Parent $PSScriptRoot
if ($PluginPath -eq '') { $PluginPath = Join-Path $repoRoot 'plugin' }
$packageJson = Join-Path $PluginPath 'package.json'
if (-not (Test-Path $packageJson)) { throw "No package.json in $PluginPath - pass -PluginPath with the package folder." }
$manifest = Get-Content $packageJson -Raw | ConvertFrom-Json
$packageName = $manifest.name
$packageVersion = $manifest.version

if ($DshHome -eq '') { $DshHome = if ($env:DSH_HOME) { $env:DSH_HOME } else { Join-Path ([Environment]::GetFolderPath('UserProfile')) '.dsh' } }
if ($ProfileName -eq '') { $ProfileName = if ($env:DSH_PROFILE) { $env:DSH_PROFILE } else { 'web' } }
$ProfileDir = Join-Path $DshHome "profiles\$ProfileName"

function Get-Npx {
    $candidate = Get-Command npx.cmd -ErrorAction SilentlyContinue
    if (-not $candidate) { $candidate = Get-Command npx -ErrorAction SilentlyContinue }
    if (-not $candidate) { throw 'npx was not found - install Node.js 22 or newer.' }
    return $candidate.Source
}

Write-Step "dsh-supercollider installer ($packageName@$packageVersion)"
Write-Note "package   $PluginPath"
Write-Note "DSH home  $DshHome"
Write-Note "profile   $ProfileName"

if (-not (Test-Path $ProfileDir)) {
    Write-Note '(the profile directory does not exist yet; the harness initializes it on add)'
}

$npx = Get-Npx
$arguments = @('--yes', '@deepseek-ai/dsh', 'plugin', '--profile', $ProfileName)
if ($Uninstall) {
    $arguments += @('remove', $packageName)
    Write-Step 'Removing from the profile'
} else {
    $arguments += @('add', (Resolve-Path $PluginPath).Path)
    Write-Step 'Adding to the profile'
}

# The harness pins its pnpm layout in the profile, and a `dsh plugin add` run
# from anywhere works out which one that is. What it does NOT appreciate is a
# different pnpm on PATH, so the environment is left as the caller had it.
$env:DSH_HOME = $DshHome
Write-Note "running: npx $($arguments -join ' ')"
& $npx @arguments
if ($LASTEXITCODE -ne 0) { throw "the harness plugin command exited $LASTEXITCODE" }

if (-not $Uninstall) {
    # The skills also ship as plain files, which is what lets an editor-less
    # profile and `npx skills add` see the same documents.
    Write-Step 'Copying the bundled skills into the harness skills root'
    $skillsRoot = Join-Path $DshHome 'skills'
    $source = Join-Path $PluginPath 'skills'
    $copied = @()
    if (Test-Path $source) {
        foreach ($skill in (Get-ChildItem $source -Directory | Sort-Object Name)) {
            $manifestFile = Join-Path $skill.FullName 'SKILL.md'
            if (-not (Test-Path $manifestFile)) { continue }
            $destination = Join-Path $skillsRoot $skill.Name
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            Copy-Item -Path (Join-Path $skill.FullName '*') -Destination $destination -Recurse -Force
            $copied += $skill.Name
        }
    }
    if ($copied.Count -gt 0) { Write-Note ("copied: " + ($copied -join ', ') + ' -> ' + $skillsRoot) }
    else { Write-Note 'no skill folders found (nothing copied)' }
}

Write-Step 'Done'
Write-Host ''
Write-Host '  Restart the DeepSeek Harness app (Ctrl+C the `npx @deepseek-ai/dsh web`'
Write-Host '  process and start it again), then hard-refresh the browser.'
Write-Host ''
Write-Host '  Requirements: SuperCollider 3.13 or newer, installed the normal way.'
Write-Host '  Nothing else: the plugin is plain JavaScript with no dependencies,'
Write-Host '  and it finds SuperCollider itself.'
Write-Host ''
Write-Host '  Then ask the agent for something in SuperCollider, or open the'
Write-Host '  SuperCollider console tab in the right bar and type a line.'
Write-Host ''
