[CmdletBinding()]
param([Parameter(Position = 0)][string] $FilePath)

$ErrorActionPreference = 'Stop'
$script:WindowsPublisherSubject = 'CN=Cadence AI (Vic) Pty Ltd, O=Cadence AI (Vic) Pty Ltd, L=Brunswick East, S=Victoria, C=AU'

function Assert-WindowsSignature {
    param([Parameter(Mandatory)][string] $FilePath)

    $signature = Get-AuthenticodeSignature -LiteralPath $FilePath
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode') {
        throw "Invalid Authenticode signature for ${FilePath}: $($signature.Status)."
    }
    if ($null -eq $signature.SignerCertificate) {
        throw "Missing signing certificate for $FilePath."
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Missing timestamp for $FilePath."
    }

    # Decode both names to compare attributes without depending on display order.
    $flags = [System.Security.Cryptography.X509Certificates.X500DistinguishedNameFlags]::UseNewLines
    $expected = [System.Security.Cryptography.X509Certificates.X500DistinguishedName]::new($script:WindowsPublisherSubject)
    $expectedFields = ($expected.Decode($flags) -split '\r?\n' | Sort-Object -CaseSensitive) -join "`n"
    $actualFields = ($signature.SignerCertificate.SubjectName.Decode($flags) -split '\r?\n' | Sort-Object -CaseSensitive) -join "`n"
    if ($actualFields -cne $expectedFields) {
        throw "Unexpected publisher for ${FilePath}: $($signature.SignerCertificate.Subject)."
    }
    Write-Information "Verified Authenticode publisher and timestamp for $FilePath" -InformationAction Continue
}

function Invoke-WindowsSigning {
    param([Parameter(Mandatory)][string] $FilePath)

    $resolvedPath = (Resolve-Path -LiteralPath $FilePath).Path
    foreach ($name in 'WINDOWS_SIGNTOOL_PATH', 'WINDOWS_SIGNING_DLIB_PATH', 'WINDOWS_SIGNING_METADATA_PATH') {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ([string]::IsNullOrWhiteSpace($value) -or -not (Test-Path -LiteralPath $value -PathType Leaf)) {
            throw "Missing Windows signing dependency: $name."
        }
    }

    & $env:WINDOWS_SIGNTOOL_PATH sign /v /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 `
        /dlib $env:WINDOWS_SIGNING_DLIB_PATH /dmdf $env:WINDOWS_SIGNING_METADATA_PATH $resolvedPath
    if ($LASTEXITCODE -ne 0) {
        throw "SignTool failed with exit code $LASTEXITCODE for $resolvedPath."
    }
    & $env:WINDOWS_SIGNTOOL_PATH verify /pa /all /tw $resolvedPath
    if ($LASTEXITCODE -ne 0) {
        throw "SignTool verification failed with exit code $LASTEXITCODE for $resolvedPath."
    }
    Assert-WindowsSignature -FilePath $resolvedPath
}

if ($MyInvocation.InvocationName -ne '.') {
    Invoke-WindowsSigning -FilePath $FilePath
}
