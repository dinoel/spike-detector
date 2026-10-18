# Builds spike-detector and installs it for the current user into
# %LOCALAPPDATA%\Programs\spike-detector: the tray monitor and its settings
# and history window. Running copies are closed first, and the monitor is
# started afterwards. No administrator rights needed.
#
# Usage: pwsh -File install.ps1 [-SkipBuild]

param(
    # Install the existing target\release build as is.
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$programs = 'spike-detector', 'spike-detector-ui'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\spike-detector'

# Rust picks the newest Visual Studio, which may lack the x64 C++ libraries,
# and vcvars picks the newest Windows SDK, which may be incomplete. Load the
# environment of an installation and SDK that have what linking needs.
function Enter-MsvcEnvironment {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return }
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) { return }
    $sdk = Get-ChildItem (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Lib') -Directory -ErrorAction SilentlyContinue |
        Where-Object { (Test-Path "$($_.FullName)\um\x64\kernel32.lib") -and (Test-Path "$($_.FullName)\ucrt\x64\ucrt.lib") } |
        Sort-Object { [version]$_.Name } -Descending |
        Select-Object -First 1
    $vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
    $environment = cmd /c "`"$vcvars`" $($sdk.Name) >nul 2>&1 && set"
    if ($LASTEXITCODE -ne 0) { throw "vcvars64.bat failed: $vcvars" }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') { Set-Item "env:$($Matches[1])" $Matches[2] }
    }
    Write-Host "Using MSVC from $vs$(if ($sdk) { ", Windows SDK $($sdk.Name)" })"
}

if (-not $SkipBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    }
    Enter-MsvcEnvironment
    cargo build --release --workspace --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
$built = $programs | ForEach-Object { Join-Path $PSScriptRoot "target\release\$_.exe" }
foreach ($exe in $built) {
    if (-not (Test-Path $exe)) { throw "Nothing to install: $exe does not exist." }
}

# Close running copies the polite way (WM_CLOSE), so the tray icon goes away.
$running = @(Get-Process -Name $programs -ErrorAction SilentlyContinue)
foreach ($process in $running) { taskkill /PID $process.Id | Out-Null }
foreach ($process in $running) {
    if (-not $process.WaitForExit(5000)) { throw "$($process.Name) (PID $($process.Id)) did not exit; close it and retry." }
}

New-Item -ItemType Directory -Force $installDir | Out-Null
Copy-Item $built $installDir -Force
$monitor = Join-Path $installDir 'spike-detector.exe'
Start-Process $monitor
Write-Host "Installed into $installDir and started the monitor."
