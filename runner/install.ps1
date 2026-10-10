# karma-team runner installer (Windows / PowerShell).
#
# Download the raw executable from your deployment, or install a local build.
# Usage: irm https://example.com/api/cli/install.ps1 | iex
# Overrides: KARMA_TEAM_API_URL, KARMA_TEAM_BIN_URL, KARMA_TEAM_LOCAL_BIN,
# KARMA_TEAM_INSTALL_DIR.

$ErrorActionPreference = 'Stop'

$apiUrl = ([string]$env:KARMA_TEAM_API_URL).TrimEnd('/')
$binUrl = [string]$env:KARMA_TEAM_BIN_URL
$localBin = [string]$env:KARMA_TEAM_LOCAL_BIN
$installDir = $env:KARMA_TEAM_INSTALL_DIR
if (-not $installDir) {
    $installDir = Join-Path $env:USERPROFILE '.local\bin'
}
$repoRoot = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { (Get-Location).Path }
$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Join-Path $repoRoot 'runner' }
$karmaTeamBin = Join-Path $installDir 'karma-team.exe'
$stagedBin = $null
$downloadDir = $null

function Assert-HttpUrl([string]$Value, [string]$Name) {
    $parsed = $null
    if (-not [Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$parsed) -or
        $parsed.Scheme -notin @('http', 'https') -or -not $parsed.Host) {
        throw "$Name must be an HTTP or HTTPS URL."
    }
}

try {
    if ($localBin -and $binUrl) {
        throw 'Set only one of KARMA_TEAM_LOCAL_BIN and KARMA_TEAM_BIN_URL.'
    }
    if ($apiUrl) { Assert-HttpUrl $apiUrl 'KARMA_TEAM_API_URL' }
    if ($binUrl) { Assert-HttpUrl $binUrl 'KARMA_TEAM_BIN_URL' }

    if ($localBin) {
        if (-not [IO.Path]::IsPathRooted($localBin)) {
            $localBin = Join-Path $repoRoot $localBin
        }
        if (-not (Test-Path -LiteralPath $localBin -PathType Leaf)) {
            throw "KARMA_TEAM_LOCAL_BIN must point to an existing executable: $localBin"
        }
        $sourceBin = $localBin
    } else {
        if (-not $binUrl -and $apiUrl) {
            $architecture = if ($env:PROCESSOR_ARCHITEW6432) {
                $env:PROCESSOR_ARCHITEW6432
            } else {
                $env:PROCESSOR_ARCHITECTURE
            }
            if ($architecture -notin @('AMD64', 'x86_64')) {
                throw "Unsupported Windows architecture for karma-team: $architecture"
            }
            $binUrl = "$apiUrl/api/cli/bin/x86_64-pc-windows-msvc/"
        }
        if ($binUrl) {
            $downloadDir = Join-Path ([IO.Path]::GetTempPath()) ("karma-team-install-" + [Guid]::NewGuid())
            New-Item -ItemType Directory -Path $downloadDir | Out-Null
            $sourceBin = Join-Path $downloadDir 'karma-team.exe'
            Write-Host '==> Downloading karma-team...'
            Invoke-WebRequest -Uri $binUrl -OutFile $sourceBin -UseBasicParsing
            if ((Get-Item -LiteralPath $sourceBin).Length -eq 0) {
                throw 'The binary download returned an empty file.'
            }
        } else {
            $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repoRoot 'target' }
            if (-not [IO.Path]::IsPathRooted($targetDir)) {
                $targetDir = Join-Path $repoRoot $targetDir
            }
            $sourceBin = @(
                (Join-Path $targetDir 'release\karma-team.exe'),
                (Join-Path $targetDir 'debug\karma-team.exe'),
                (Join-Path $scriptDir 'target\release\karma-team.exe'),
                (Join-Path $scriptDir 'target\debug\karma-team.exe'),
                $karmaTeamBin
            ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
            if (-not $sourceBin) {
                $manifest = Join-Path $scriptDir 'Cargo.toml'
                if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
                    throw 'No local source tree found. Set KARMA_TEAM_API_URL to your deployment URL.'
                }
                Write-Host '==> Building karma-team from local source...'
                & cargo build --manifest-path $manifest --target-dir $targetDir --locked --bin karma-team --release
                if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
                $sourceBin = Join-Path $targetDir 'release\karma-team.exe'
            }
        }
    }

    if (-not (Test-Path -LiteralPath $sourceBin -PathType Leaf)) {
        throw "karma-team.exe was not found at: $sourceBin"
    }
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    if ([IO.Path]::GetFullPath($sourceBin) -ne [IO.Path]::GetFullPath($karmaTeamBin)) {
        # Keep staging on the destination volume so the final replacement is atomic.
        $stagedBin = "$karmaTeamBin.tmp.$([Guid]::NewGuid())"
        Copy-Item -LiteralPath $sourceBin -Destination $stagedBin
        if (Test-Path -LiteralPath $karmaTeamBin) {
            [IO.File]::Replace($stagedBin, $karmaTeamBin, $null)
        } else {
            [IO.File]::Move($stagedBin, $karmaTeamBin)
        }
        $stagedBin = $null
    }
} finally {
    if ($stagedBin -and (Test-Path -LiteralPath $stagedBin)) {
        Remove-Item -LiteralPath $stagedBin -Force
    }
    if ($downloadDir -and (Test-Path -LiteralPath $downloadDir)) {
        Remove-Item -LiteralPath $downloadDir -Recurse -Force
    }
}

Write-Host "Installed karma-team at $karmaTeamBin"
if ($installDir -notin ($env:PATH -split ';')) {
    $env:PATH = "$installDir;$env:PATH"
    Write-Host "Add $installDir to your user PATH to use karma-team in new terminals."
}

if (-not [Environment]::UserInteractive -or [Console]::IsInputRedirected) {
    Write-Host 'No interactive console detected - skipping auto-auth.'
    $quotedBin = "'" + $karmaTeamBin.Replace("'", "''") + "'"
    $quotedUrl = if ($apiUrl) { "'" + $apiUrl.Replace("'", "''") + "'" } else { "'<URL>'" }
    Write-Host "Run: & $quotedBin auth login --no-browser --url $quotedUrl"
    Write-Host 'Approve the printed URL in a browser, then run karma-team runner add --project <PROJECT>.'
} else {
    Write-Host '==> Starting authentication...'
    if ($apiUrl) {
        & $karmaTeamBin auth login --url $apiUrl
    } else {
        & $karmaTeamBin auth login
    }
    if ($LASTEXITCODE -ne 0) { throw "karma-team auth login exited with code $LASTEXITCODE" }
}
