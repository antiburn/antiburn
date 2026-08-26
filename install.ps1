# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

[CmdletBinding()]
param(
    [Parameter()]
    [ValidatePattern('^[0-9A-Za-z.-]+$')]
    [string] $Version = $env:ANTIBURN_VERSION
)

$ErrorActionPreference = 'Stop'
$script:Repository = 'antiburn/antiburn'
$script:GitHubUrl = "https://github.com/$script:Repository"

function Write-InstallerInfo {
    param([Parameter(Mandatory)][string] $Message)
    Write-Host "antiburn: $Message"
}

function Get-AntiburnRelease {
    param([string] $RequestedVersion)

    if ($RequestedVersion) {
        return [PSCustomObject]@{
            Version = $RequestedVersion
            Tag = "antiburn-v$RequestedVersion"
        }
    }

    Write-InstallerInfo 'Resolving the latest release'
    $request = @{
        Uri = "https://api.github.com/repos/$script:Repository/releases/latest"
        Headers = @{ Accept = 'application/vnd.github+json' }
        UseBasicParsing = $true
    }
    $release = Invoke-RestMethod @request
    $tag = [string] $release.tag_name
    if ($tag -notmatch '^antiburn-v([0-9A-Za-z.-]+)$') {
        throw "GitHub returned an invalid release tag: $tag"
    }
    return [PSCustomObject]@{
        Version = $Matches[1]
        Tag = $tag
    }
}

function Invoke-InstallerDownload {
    param(
        [Parameter(Mandatory)][uri] $Uri,
        [Parameter(Mandatory)][string] $OutFile
    )

    $request = @{
        Uri = $Uri
        UseBasicParsing = $true
    }
    if ($Uri.Scheme -cne 'https') {
        throw "Refusing a non-HTTPS download: $Uri"
    }
    $response = Invoke-WebRequest @request
    $finalUri = if ($response.BaseResponse.ResponseUri) {
        $response.BaseResponse.ResponseUri
    }
    else {
        $response.BaseResponse.RequestMessage.RequestUri
    }
    if ($finalUri.Scheme -cne 'https') {
        throw "Refusing a download that redirected to $finalUri"
    }
    $outputStream = $null
    try {
        $outputStream = [System.IO.File]::Create($OutFile)
        $response.RawContentStream.CopyTo($outputStream)
    }
    finally {
        if ($null -ne $outputStream) {
            $outputStream.Dispose()
        }
        $response.RawContentStream.Dispose()
    }
}

function Get-ExpectedChecksum {
    param(
        [Parameter(Mandatory)][string] $ChecksumFile,
        [Parameter(Mandatory)][string] $AssetName
    )

    $matches = foreach ($line in Get-Content -LiteralPath $ChecksumFile) {
        if ($line -match '^([0-9A-Fa-f]{64})\s+\*?(.+)$' -and $Matches[2] -ceq $AssetName) {
            $Matches[1].ToLowerInvariant()
        }
    }
    if (@($matches).Count -ne 1) {
        throw "SHA256SUMS must contain exactly one valid entry for $AssetName."
    }
    return $matches
}

function Assert-InstallerIntegrity {
    param(
        [Parameter(Mandatory)][string] $InstallerPath,
        [Parameter(Mandatory)][string] $ChecksumFile
    )

    $assetName = Split-Path -Leaf $InstallerPath
    $expected = Get-ExpectedChecksum -ChecksumFile $ChecksumFile -AssetName $assetName
    $actual = (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -cne $expected) {
        throw "Checksum verification failed for $assetName."
    }
    Write-InstallerInfo "Verified SHA-256 for $assetName"
}

function Test-WindowsArchitecture {
    $architecture = if ($env:PROCESSOR_ARCHITEW6432) {
        $env:PROCESSOR_ARCHITEW6432
    }
    else {
        $env:PROCESSOR_ARCHITECTURE
    }
    if ($architecture -notin @('AMD64', 'x86_64')) {
        throw "Unsupported Windows architecture: $architecture"
    }
}

function Invoke-AntiburnInstall {
    param([string] $RequestedVersion)

    if ($PSVersionTable.PSVersion.Major -lt 6) {
        $protocol = [Net.ServicePointManager]::SecurityProtocol
        [Net.ServicePointManager]::SecurityProtocol = $protocol -bor [Net.SecurityProtocolType]::Tls12
    }

    Test-WindowsArchitecture
    $release = Get-AntiburnRelease -RequestedVersion $RequestedVersion
    $assetName = "antiburn_$($release.Version)_x64-setup.exe"
    $baseUrl = "$script:GitHubUrl/releases/download/$($release.Tag)"
    $temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("antiburn-install-" + [guid]::NewGuid())
    $installerPath = Join-Path $temporaryDirectory $assetName
    $checksumPath = Join-Path $temporaryDirectory 'SHA256SUMS'

    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    try {
        Write-InstallerInfo "Downloading $assetName"
        Invoke-InstallerDownload -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumPath
        Invoke-InstallerDownload -Uri "$baseUrl/$assetName" -OutFile $installerPath
        Assert-InstallerIntegrity -InstallerPath $installerPath -ChecksumFile $checksumPath

        if ($env:ANTIBURN_VERIFY_ATTESTATION -eq '1') {
            if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
                throw 'gh is required when ANTIBURN_VERIFY_ATTESTATION=1.'
            }
            & gh release verify-asset $release.Tag $installerPath --repo $script:Repository | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "GitHub could not verify the release attestation for $assetName."
            }
            Write-InstallerInfo 'Verified the GitHub release attestation'
        }

        Write-Warning 'The Windows installer is not required to have an Authenticode signature yet. Windows SmartScreen can warn.'
        Write-InstallerInfo 'Starting the passive installer'
        $process = Start-Process -FilePath $installerPath -ArgumentList '/P /R' -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            throw "The Windows installer failed with exit code $($process.ExitCode)."
        }

        $installedApplication = Join-Path $env:LOCALAPPDATA 'antiburn\antiburn.exe'
        if (-not (Test-Path -LiteralPath $installedApplication)) {
            throw "The installer completed, but antiburn was not found at $installedApplication."
        }
        Write-InstallerInfo "Installed antiburn $($release.Version)"
    }
    finally {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    Invoke-AntiburnInstall -RequestedVersion $Version
}
