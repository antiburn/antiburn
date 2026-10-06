[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Expand-VerifiedSigningDownload {
    param(
        [Parameter(Mandatory)][uri] $Uri,
        [Parameter(Mandatory)][string] $Destination,
        [Parameter(Mandatory)][string] $Hash,
        [Parameter()][ValidateSet('SHA256', 'SHA512')][string] $Algorithm = 'SHA256'
    )

    $archive = "$Destination.zip"
    Invoke-WebRequest -Uri $Uri -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm $Algorithm).Hash -ine $Hash) {
        throw "Signing dependency checksum failed for $Uri."
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $Destination -Force
    Remove-Item -LiteralPath $archive
}

function Install-WindowsSigningToolchain {
    param([Parameter(Mandatory)][string] $Directory)

    New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    $sdk = Join-Path $Directory 'sdk'
    $client = Join-Path $Directory 'client'
    $runtime = Join-Path $Directory 'dotnet'
    Expand-VerifiedSigningDownload `
        -Uri 'https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.buildtools/10.0.26100.4188/microsoft.windows.sdk.buildtools.10.0.26100.4188.nupkg' `
        -Destination $sdk -Hash '180deb372659029864c10a0c04787833234d64aacd1d2c0661d2c00295d8e022'
    Expand-VerifiedSigningDownload `
        -Uri 'https://api.nuget.org/v3-flatcontainer/microsoft.artifactsigning.client/1.0.128/microsoft.artifactsigning.client.1.0.128.nupkg' `
        -Destination $client -Hash '74bd7d27e6ce1051409c38d9b46bc8df0400ecd643d51ffbf2ac00869061e40b'
    Expand-VerifiedSigningDownload `
        -Uri 'https://builds.dotnet.microsoft.com/dotnet/Runtime/8.0.31/dotnet-runtime-8.0.31-win-x64.zip' `
        -Destination $runtime -Algorithm SHA512 `
        -Hash '9c55c58694676ee64b0eed2cd6d8cbf58b9aa8288420acc66841e15ca0099c75d4af0182d23a641c2342e5a151a325df4a12fa0bde2e47c0fb7e9a33e7b09896'

    # The client DLL requires an x64 runtime, including on Windows ARM64.
    $env:DOTNET_ROOT = $runtime
    $env:DOTNET_ROOT_X64 = $runtime
    $signTool = Join-Path $sdk 'bin/10.0.26100.0/x64/signtool.exe'
    $dlib = Join-Path $client 'bin/x64/Azure.CodeSigning.Dlib.dll'
    foreach ($path in $signTool, $dlib) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Signing dependency is missing: $path."
        }
    }
    & (Join-Path $runtime 'dotnet.exe') --list-runtimes | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "The x64 signing runtime failed with exit code $LASTEXITCODE."
    }
    & $signTool verify /pa (Join-Path $runtime 'dotnet.exe') | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "The x64 SignTool check failed with exit code $LASTEXITCODE."
    }
    return @{
        WINDOWS_SIGNTOOL_PATH       = $signTool
        TAURI_WINDOWS_SIGNTOOL_PATH = $signTool
        WINDOWS_SIGNING_DLIB_PATH   = $dlib
        DOTNET_ROOT                 = $runtime
        DOTNET_ROOT_X64             = $runtime
    }
}

function Initialize-WindowsSigningConfiguration {
    param(
        [Parameter(Mandatory)][string] $Directory,
        [Parameter(Mandatory)][uri] $Endpoint,
        [Parameter(Mandatory)][string] $AccountName,
        [Parameter(Mandatory)][string] $CertificateProfileName
    )

    if ($Endpoint.Scheme -cne 'https' -or $Endpoint.Host -notmatch '^[a-z0-9]+\.codesigning\.azure\.net$') {
        throw 'The signing endpoint must be an HTTPS Azure code-signing endpoint.'
    }
    $metadataPath = Join-Path $Directory 'metadata.json'
    @{
        Endpoint               = $Endpoint.AbsoluteUri
        CodeSigningAccountName = $AccountName
        CertificateProfileName = $CertificateProfileName
        ExcludeCredentials     = @(
            'EnvironmentCredential', 'WorkloadIdentityCredential', 'ManagedIdentityCredential',
            'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential',
            'AzurePowerShellCredential', 'AzureDeveloperCliCredential', 'InteractiveBrowserCredential'
        )
    } | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $metadataPath -Encoding utf8

    $configPath = Join-Path $Directory 'signing.conf.json'
    $wrapper = Join-Path $PSScriptRoot 'windows-signing.ps1'
    @{
        bundle = @{
            windows = @{
                signCommand = @{
                    cmd  = (Get-Command pwsh -CommandType Application).Source
                    args = @('-NoProfile', '-NonInteractive', '-File', $wrapper, '%1')
                }
            }
        }
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $configPath -Encoding utf8
    return @{
        WINDOWS_SIGNING_METADATA_PATH = $metadataPath
        TAURI_SIGNING_CONFIG          = $configPath
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    $directory = Join-Path $env:RUNNER_TEMP 'windows-signing'
    $tools = Install-WindowsSigningToolchain -Directory $directory
    $configuration = Initialize-WindowsSigningConfiguration -Directory $directory `
        -Endpoint $env:AZURE_SIGNING_ENDPOINT -AccountName $env:AZURE_SIGNING_ACCOUNT_NAME `
        -CertificateProfileName $env:AZURE_SIGNING_CERTIFICATE_PROFILE_NAME
    foreach ($values in $tools, $configuration) {
        foreach ($entry in $values.GetEnumerator()) {
            "$($entry.Key)=$($entry.Value)" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
        }
    }
}
