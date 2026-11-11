# cc-sessions installer for Windows.
#
#   irm https://raw.githubusercontent.com/barath-dev/cc-sessions/main/install.ps1 | iex
#
# Downloads the one program for this PC, checks it against the release's
# SHA-256 list, puts it in %LOCALAPPDATA%\cc-sessions, and runs its setup.
# Running it again later updates the program and keeps all settings.
# To pass setup options:
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/barath-dev/cc-sessions/main/install.ps1))) --yes --command cs

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = 'barath-dev/cc-sessions'
$base = if ($env:CC_SESSIONS_INSTALL_BASE) { $env:CC_SESSIONS_INSTALL_BASE } else { "https://github.com/$repo/releases/latest/download" }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x86_64' }
$asset = "cc-sessions-windows-$arch.exe"

$tmp = Join-Path ([IO.Path]::GetTempPath()) "cc-sessions-install-$PID"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    Write-Host "Downloading $asset..."
    $download = Join-Path $tmp $asset
    $sums = Join-Path $tmp 'checksums.txt'
    Invoke-WebRequest -UseBasicParsing "$base/$asset" -OutFile $download
    Invoke-WebRequest -UseBasicParsing "$base/checksums.txt" -OutFile $sums

    $expected = Get-Content $sums |
        Where-Object { ($_ -split '\s+')[1] -in @($asset, "*$asset") } |
        ForEach-Object { ($_ -split '\s+')[0] } |
        Select-Object -First 1
    $actual = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLower()
    if (-not $expected -or $expected.ToLower() -ne $actual) {
        throw 'cc-sessions: the download does not match the release checksum. Nothing was installed.'
    }

    $dir = Join-Path $env:LOCALAPPDATA 'cc-sessions'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $exe = Join-Path $dir 'cc-sessions.exe'
    if (Test-Path $exe) {
        # Windows keeps a running program's file locked, so stop the server before replacing it.
        & $exe stop | Out-Null
    }
    Move-Item -Force $download $exe
    Write-Host "Installed $(& $exe --version) in $dir"
    Write-Host ''

    & $exe setup @args
    if ($LASTEXITCODE -ne 0) { throw 'cc-sessions: setup did not finish.' }
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
