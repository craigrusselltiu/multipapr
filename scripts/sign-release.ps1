[CmdletBinding()]
param([string]$Binary = (Join-Path $PSScriptRoot '..\target\release\multipapr.exe'))
$ErrorActionPreference = 'Stop'
$path = (Resolve-Path -LiteralPath $Binary).ProviderPath
$tag = 'v' + [Diagnostics.FileVersionInfo]::GetVersionInfo($path).ProductVersion
if ($tag -cnotmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') { throw 'Invalid release version.' }

$key = [Security.Cryptography.CngKey]::Open('Multipapr Release Signing v1')
try {
    $pinned = [IO.File]::ReadAllBytes((Join-Path $PSScriptRoot '..\assets\release-key.blob'))
    $public = $key.Export([Security.Cryptography.CngKeyBlobFormat]::EccPublicBlob)
    if ([Convert]::ToBase64String($pinned) -cne [Convert]::ToBase64String($public)) {
        throw 'This signing key does not match the Multipapr publisher key.'
    }
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    $message = [Text.Encoding]::UTF8.GetBytes("Multipapr Windows x64`n$tag`n$hash`n")
    $signer = [Security.Cryptography.ECDsaCng]::new($key)
    try {
        $signature = $signer.SignData($message, [Security.Cryptography.HashAlgorithmName]::SHA256)
        [IO.File]::WriteAllBytes("$path.sig", $signature)
    } finally { $signer.Dispose() }
} finally { $key.Dispose() }
& (Join-Path $PSScriptRoot '..\install.ps1') -VerifyFile $path -Version $tag
