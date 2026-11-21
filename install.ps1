[CmdletBinding()]
param(
    [switch]$FromSource,
    [switch]$FromRelease,
    [string]$Version = 'latest'
)
$ErrorActionPreference = 'Stop'
$repo = 'craigrusselltiu/multipapr'
$installDir = Join-Path $env:LOCALAPPDATA 'Multipapr'
$installed = Join-Path $installDir 'multipapr.exe'
$legacyMpvDir = Join-Path $installDir 'tools\mpv'

$localRepo = $PSScriptRoot -and (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'Cargo.toml'))
$buildFromSource = $FromSource -or ($localRepo -and -not $FromRelease)

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$tempBinary = $null

try {
    if ($buildFromSource) {
        if (-not $localRepo) {
            throw 'Run .\install.ps1 -FromSource from a cloned multipapr repository.'
        }
        if (-not $ExecutionContext.InvokeCommand.GetCommand('cargo', [System.Management.Automation.CommandTypes]::All)) {
            throw 'Missing cargo. Install Rust (https://rustup.rs) or run with -FromRelease.'
        }
        Push-Location $PSScriptRoot
        try {
            cargo build --release --locked
            if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
            $metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
            $binary = Join-Path $metadata.target_directory 'release\multipapr.exe'
            if (-not (Test-Path -LiteralPath $binary)) { throw "Missing release binary at $binary" }
        } finally { Pop-Location }
    } else {
        if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
            throw 'Prebuilt binaries currently target Windows x64. Build from source on other architectures.'
        }
        $releaseBase = if ($Version -eq 'latest') {
            "https://github.com/$repo/releases/latest/download"
        } else {
            "https://github.com/$repo/releases/download/$Version"
        }
        $tempBinary = Join-Path ([IO.Path]::GetTempPath()) ("multipapr-" + [guid]::NewGuid() + ".exe")
        Write-Host "Downloading Multipapr ($Version)..." -ForegroundColor Cyan
        Invoke-WebRequest -UseBasicParsing "$releaseBase/multipapr.exe" -OutFile $tempBinary
        $binary = $tempBinary
    }

    $hosts = @([Diagnostics.Process]::GetProcessesByName('multipapr') | Where-Object Path -eq $installed)
    if ($hosts) {
        Start-Process -FilePath $installed -ArgumentList '--stop' -WindowStyle Hidden -Wait
        foreach ($process in $hosts) {
            if (-not $process.WaitForExit(4000)) {
                throw 'Close the Multipapr editor and rerun the installer.'
            }
        }
    }

    if (Test-Path -LiteralPath $legacyMpvDir) {
        Remove-Item -LiteralPath $legacyMpvDir -Recurse -Force -ErrorAction Continue
    }

    Copy-Item -LiteralPath $binary -Destination $installed -Force

    $iconPath = $installed
    if ($localRepo) {
        $localIcon = Join-Path $PSScriptRoot 'multipapr.ico'
        if (Test-Path -LiteralPath $localIcon) {
            $iconPath = Join-Path $installDir 'fold.ico'
            Copy-Item -LiteralPath $localIcon -Destination $iconPath -Force
        }
        $fontLicense = Join-Path $PSScriptRoot 'assets/fonts/LICENSE.txt'
        if (Test-Path -LiteralPath $fontLicense) {
            Copy-Item -LiteralPath $fontLicense -Destination (Join-Path $installDir 'Inter-LICENSE.txt') -Force
        }
    }

    $programs = [Environment]::GetFolderPath('Programs')
    $link = Join-Path $programs 'Multipapr.lnk'
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($link)
    $shortcut.TargetPath = $installed
    $shortcut.WorkingDirectory = $installDir
    $shortcut.IconLocation = "$iconPath,0"
    $shortcut.Save()

    if (-not ('Multipapr.Shell' -as [type])) {
        Add-Type -Namespace Multipapr -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern void SHChangeNotify(int change, uint flags, string path, System.IntPtr other);
'@
    }
    $shcneUpdateItem = 0x2000
    $shcnfPathW = 0x0005
    $shcnfFlush = 0x1000
    foreach ($item in $iconPath, $installed, $link) {
        [Multipapr.Shell]::SHChangeNotify($shcneUpdateItem, ($shcnfPathW -bor $shcnfFlush), $item, [IntPtr]::Zero)
    }

    if ($hosts -and (Test-Path -LiteralPath (Join-Path $installDir 'layout.json'))) {
        Start-Process -FilePath $installed -ArgumentList '--background' -WindowStyle Hidden
    }

    Write-Host "Installed $installed" -ForegroundColor Green
    if (-not $hosts) {
        Write-Host 'Launch Multipapr from the Start Menu to set up your wallpaper.' -ForegroundColor Gray
    }
} finally {
    if ($tempBinary -and (Test-Path -LiteralPath $tempBinary)) {
        Remove-Item -LiteralPath $tempBinary -Force
    }
}
