[CmdletBinding()]
param(
    [switch]$FromSource,
    [switch]$FromRelease,
    [ValidatePattern('^(latest|v[0-9]+\.[0-9]+\.[0-9]+)$')]
    [string]$Version = 'latest',
    [string]$VerifyFile
)
$ErrorActionPreference = 'Stop'
$repo = 'craigrusselltiu/multipapr'
$installDir = Join-Path $env:LOCALAPPDATA 'Multipapr'
$installed = Join-Path $installDir 'multipapr.exe'
$legacyMpvDir = Join-Path $installDir 'tools\mpv'

function Open-VerifiedRelease([string]$Path, [string]$ExpectedVersion) {
    $publicKey = 'RUNTMSAAAADbznhFo8W6lgXHZouchRXo7OWrIXA/hz7dymsvvYK9C6tv5AT356ndYrytLk+yox9tfCtmw3DOx+96i1jeRbFM'
    $stream = [IO.File]::Open($Path, 'Open', 'Read', 'Read')
    try {
        if ((Get-Item -LiteralPath "$Path.sig").Length -ne 64) { throw 'Invalid Multipapr signature length.' }
        $signature = [IO.File]::ReadAllBytes("$Path.sig")
        $versionText = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path).ProductVersion
        $tag = "v$versionText"
        if ($tag -cnotmatch '^v[0-9]+\.[0-9]+\.[0-9]+$' -or
            ($ExpectedVersion -ne 'latest' -and $tag -cne $ExpectedVersion)) {
            throw 'Unexpected Multipapr release version.'
        }
        $hash = (Get-FileHash -InputStream $stream -Algorithm SHA256).Hash.ToLowerInvariant()
        $message = [Text.Encoding]::UTF8.GetBytes("Multipapr Windows x64`n$tag`n$hash`n")
        $key = [Security.Cryptography.CngKey]::Import([Convert]::FromBase64String($publicKey), [Security.Cryptography.CngKeyBlobFormat]::EccPublicBlob)
        try {
            $verifier = [Security.Cryptography.ECDsaCng]::new($key)
            try {
                if (-not $verifier.VerifyData($message, $signature, [Security.Cryptography.HashAlgorithmName]::SHA256)) {
                    throw 'Multipapr publisher signature did not verify.'
                }
            } finally { $verifier.Dispose() }
        } finally { $key.Dispose() }
        $stream.Position = 0
        return $stream
    } catch { $stream.Dispose(); throw }
}

if ($VerifyFile) {
    $path = (Resolve-Path -LiteralPath $VerifyFile).ProviderPath
    (Open-VerifiedRelease $path $Version).Dispose()
    Write-Host "Verified Multipapr publisher signature for $path."
    return
}

$localRepo = $PSScriptRoot -and (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'Cargo.toml'))
$buildFromSource = $FromSource -or ($localRepo -and -not $FromRelease)

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$tempBinary = $null
$verifiedStream = $null
$staged = Join-Path $installDir ("multipapr-" + [guid]::NewGuid() + ".tmp")

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
        Invoke-WebRequest -UseBasicParsing "$releaseBase/multipapr.exe.sig" -OutFile "$tempBinary.sig"
        Invoke-WebRequest -UseBasicParsing "$releaseBase/multipapr.exe" -OutFile $tempBinary
        $verifiedStream = Open-VerifiedRelease $tempBinary $Version
        $binary = $tempBinary
    }

    [IO.File]::Copy($binary, $staged)
    $hosts = @([Diagnostics.Process]::GetProcessesByName('multipapr') | Where-Object Path -eq $installed)
    if ($hosts) {
        try {
            $stop = [Threading.EventWaitHandle]::OpenExisting('Local\Multipapr.Stop.v1')
            try { [void]$stop.Set() } finally { $stop.Dispose() }
        } catch [Threading.WaitHandleCannotBeOpenedException] {
            # A standalone editor has no host event. Allow it to exit below.
        }
        foreach ($process in $hosts) {
            if (-not $process.WaitForExit(4000)) {
                throw 'Close the Multipapr editor and rerun the installer.'
            }
        }
    }

    if (Test-Path -LiteralPath $legacyMpvDir) {
        Remove-Item -LiteralPath $legacyMpvDir -Recurse -Force -ErrorAction Continue
    }

    if ([IO.File]::Exists($installed)) {
        [IO.File]::Replace($staged, $installed, [System.Management.Automation.Language.NullString]::Value)
    } else {
        [IO.File]::Move($staged, $installed)
    }

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
    if ($verifiedStream) { $verifiedStream.Dispose() }
    if ([IO.File]::Exists($staged)) { [IO.File]::Delete($staged) }
    if ($tempBinary -and (Test-Path -LiteralPath "$tempBinary.sig")) {
        Remove-Item -LiteralPath "$tempBinary.sig" -Force
    }
    if ($tempBinary -and (Test-Path -LiteralPath $tempBinary)) {
        Remove-Item -LiteralPath $tempBinary -Force
    }
}
