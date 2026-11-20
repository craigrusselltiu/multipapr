# Build and install for the current Windows user.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$installDir = Join-Path $env:LOCALAPPDATA 'Multipapr'
$installed = Join-Path $installDir 'multipapr.exe'
$mpvDir = Join-Path $installDir 'tools\mpv'

foreach ($tool in 'cargo','ffmpeg','ffprobe') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "Missing $tool. Install Rust and FFmpeg, then reopen PowerShell."
    }
}
Push-Location $PSScriptRoot
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
    $metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $binary = Join-Path $metadata.target_directory 'release\multipapr.exe'
    if (-not (Test-Path -LiteralPath $binary)) { throw "Missing release binary at $binary" }
    New-Item -ItemType Directory -Force $installDir | Out-Null
    if (-not (Test-Path -LiteralPath (Join-Path $mpvDir 'mpv.exe')) -and -not (Get-Command mpv.exe -ErrorAction SilentlyContinue)) {
        if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw 'Install mpv for your architecture and put mpv.exe on PATH.' }
        $archive = Join-Path ([IO.Path]::GetTempPath()) ("multipapr-" + [guid]::NewGuid() + ".7z")
        try {
            $url = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-x86_64-20261111-git-bcde6b93cc.7z'
            Invoke-WebRequest -UseBasicParsing $url -OutFile $archive
            if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne 'f259b5fd4b5c395617c948a4fadcdf55bba1c5282fb7c13e88626b3f71072baf') {
                throw 'mpv archive checksum mismatch'
            }
            New-Item -ItemType Directory -Force $mpvDir | Out-Null
            tar.exe -xf $archive -C $mpvDir
            if ($LASTEXITCODE -ne 0) { throw 'mpv extraction failed' }
        } finally { if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive } }
    }
    $hosts = @(Get-Process multipapr -ErrorAction SilentlyContinue | Where-Object Path -eq $installed)
    if ($hosts) {
        Start-Process -FilePath $installed -ArgumentList '--stop' -WindowStyle Hidden -Wait
        foreach ($process in $hosts) {
            if (-not $process.WaitForExit(4000)) { throw 'Close the Multipapr editor and rerun the installer.' }
        }
    }
    Copy-Item -LiteralPath $binary -Destination $installed -Force
    $icon = Join-Path $installDir 'fold.ico'
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'multipapr.ico') -Destination $icon -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'assets/fonts/LICENSE.txt') -Destination (Join-Path $installDir 'Inter-LICENSE.txt') -Force
    $programs = [Environment]::GetFolderPath('Programs')
    $link = Join-Path $programs 'Multipapr.lnk'
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($link)
    $shortcut.TargetPath = $installed
    $shortcut.WorkingDirectory = $installDir
    $shortcut.IconLocation = "$icon,0"
    $shortcut.Save()
    if (-not ('Multipapr.Shell' -as [type])) {
        Add-Type -Namespace Multipapr -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern void SHChangeNotify(int change, uint flags, string path, System.IntPtr other);
'@
    }
    # SHCNE_UPDATEITEM with SHCNF_PATHW | SHCNF_FLUSH refreshes these items in Explorer.
    foreach ($item in $icon, $installed, $link) {
        [Multipapr.Shell]::SHChangeNotify(0x2000, 0x1005, $item, [IntPtr]::Zero)
    }
    if ($hosts -and (Test-Path -LiteralPath (Join-Path $installDir 'layout.json'))) {
        Start-Process -FilePath $installed -ArgumentList '--background' -WindowStyle Hidden
    }
    Write-Output "Installed $installed"
} finally { Pop-Location }
