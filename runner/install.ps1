# karma-team runner installer (Windows / PowerShell).
#
# Windows wrapper for the cargo-dist-generated installer. Wraps
# `karma-team-installer.ps1`, then launches `karma-team auth login` so the user
# lands in the device-code flow on the same install one-liner. The
# runner is a "set up once, forget" daemon driven by karma-team cloud, so
# the natural moment to authenticate is right now while the user is
# at the terminal — not the first time they happen to type `karma-team`
# themselves (which may be never).
#
# Usage:
#   irm https://github.com/xerecter/karma-team/releases/latest/download/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$installerUrl = 'https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.ps1'

Write-Host '==> Downloading karma-team...'
Invoke-Expression (Invoke-RestMethod $installerUrl)

# cargo-dist drops karma-team.exe into $env:USERPROFILE\.local\bin (the
# install-path = "$HOME/.local/bin" in dist-workspace.toml resolves
# to USERPROFILE on Windows). If a future release moves it, surface
# a clear error instead of silently continuing.
$karmaTeamBin = Join-Path $env:USERPROFILE '.local\bin\karma-team.exe'
if (-not (Test-Path -PathType Leaf $karmaTeamBin)) {
    Write-Host ''
    Write-Host "karma-team.exe not found at $karmaTeamBin after install."
    Write-Host "Run ``$installerUrl`` manually, then ``karma-team auth login``."
    exit 1
}

Write-Host ''
Write-Host '==> Starting authentication...'
Write-Host ''

# Headless detection: in non-interactive contexts (Windows scheduled
# tasks, Packer/Ansible provisioners, MSI ExecuteSequence, CI on a
# Windows runner without a desktop session) there's no one to approve
# the device code. Skip the auto-launch and point at the headless
# enrollment path instead.
#
# Unix's /dev/tty reattach trick has no Windows equivalent and isn't
# needed: PowerShell child processes inherit the console host directly,
# so even when our own stdin is the `irm | iex` pipe, `karma-team auth
# login` still reads keystrokes from the user's console.
if (-not [Environment]::UserInteractive) {
    Write-Host 'No interactive console detected — skipping auto-auth.'
    Write-Host 'Run `karma-team auth login --no-browser --url <URL>`, approve the printed'
    Write-Host 'URL from another browser, then run `karma-team runner add --project <PROJECT>`.'
    exit 0
}

& $karmaTeamBin auth login
exit $LASTEXITCODE
