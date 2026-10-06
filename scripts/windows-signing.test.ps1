BeforeAll {
    . (Join-Path $PSScriptRoot 'windows-signing.ps1')
    . (Join-Path $PSScriptRoot 'setup-windows-signing.ps1')

    if (-not (Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue)) {
        function Get-AuthenticodeSignature { param([string] $LiteralPath) }
    }
    function New-SignatureFixture {
        param([string] $Subject = $script:WindowsPublisherSubject)
        return [PSCustomObject]@{
            Status                 = 'Valid'
            SignatureType          = 'Authenticode'
            SignerCertificate      = [PSCustomObject]@{
                Subject     = $Subject
                SubjectName = [System.Security.Cryptography.X509Certificates.X500DistinguishedName]::new($Subject)
                NotAfter    = [datetime]::UtcNow.AddDays(-1)
            }
            TimeStamperCertificate = [PSCustomObject]@{ Subject = 'CN=Synthetic timestamp' }
        }
    }
}

Describe 'Windows Authenticode verification' {
    BeforeEach {
        $script:SignatureFixture = New-SignatureFixture
        Mock Get-AuthenticodeSignature { $script:SignatureFixture }
    }

    It 'accepts the expected publisher with a valid timestamp after leaf expiry' {
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Not -Throw
        Should -Invoke Get-AuthenticodeSignature -Times 1 -Exactly -ParameterFilter {
            $LiteralPath -eq 'synthetic.exe'
        }
    }

    It 'accepts equivalent subject formatting and reversed attribute order' {
        $script:SignatureFixture = New-SignatureFixture -Subject 'C=AU, ST=Victoria, L=Brunswick East, O=Cadence AI (Vic) Pty Ltd, CN=Cadence AI (Vic) Pty Ltd'
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Not -Throw
    }

    It 'rejects a <Status> signature' -ForEach @(
        @{ Status = 'NotSigned' }
        @{ Status = 'HashMismatch' }
        @{ Status = 'NotTrusted' }
        @{ Status = 'UnknownError' }
    ) {
        $script:SignatureFixture.Status = $Status
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Invalid Authenticode signature*'
    }

    It 'rejects a catalog signature' {
        $script:SignatureFixture.SignatureType = 'Catalog'
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Invalid Authenticode signature*'
    }

    It 'rejects a missing signing certificate' {
        $script:SignatureFixture.SignerCertificate = $null
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Missing signing certificate*'
    }

    It 'rejects a missing timestamp' {
        $script:SignatureFixture.TimeStamperCertificate = $null
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Missing timestamp*'
    }

    It 'rejects a matching CN with a different organization' {
        $script:SignatureFixture = New-SignatureFixture -Subject 'CN=Cadence AI (Vic) Pty Ltd, O=Synthetic Other Company, L=Brunswick East, S=Victoria, C=AU'
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Unexpected publisher*'
    }

    It 'rejects a publisher that contains the expected name as a substring' {
        $script:SignatureFixture = New-SignatureFixture -Subject 'CN=Cadence AI (Vic) Pty Ltd Other, O=Cadence AI (Vic) Pty Ltd, L=Brunswick East, S=Victoria, C=AU'
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Unexpected publisher*'
    }

    It 'rejects an additional subject attribute' {
        $script:SignatureFixture = New-SignatureFixture -Subject "$script:WindowsPublisherSubject, OU=Synthetic"
        { Assert-WindowsSignature -FilePath 'synthetic.exe' } | Should -Throw '*Unexpected publisher*'
    }
}

Describe 'Windows signing command' {
    BeforeEach {
        $script:PreviousSigningEnvironment = @{}
        foreach ($name in 'WINDOWS_SIGNTOOL_PATH', 'WINDOWS_SIGNING_DLIB_PATH', 'WINDOWS_SIGNING_METADATA_PATH') {
            $script:PreviousSigningEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
            [Environment]::SetEnvironmentVariable($name, '')
        }
        $script:SyntheticBinary = Join-Path $TestDrive 'synthetic app.exe'
        Set-Content -LiteralPath $script:SyntheticBinary -Value 'synthetic executable'
    }

    AfterEach {
        foreach ($entry in $script:PreviousSigningEnvironment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
        }
    }

    It 'fails before signing when dependencies are missing' {
        { Invoke-WindowsSigning -FilePath $script:SyntheticBinary } | Should -Throw '*Missing Windows signing dependency*'
    }

    It 'propagates a signing failure without running signature verification' {
        $tool = Join-Path $TestDrive 'synthetic-signtool.ps1'
        Set-Content -LiteralPath $tool -Value '$global:LASTEXITCODE = 1'
        $env:WINDOWS_SIGNTOOL_PATH = $tool
        $env:WINDOWS_SIGNING_DLIB_PATH = $script:SyntheticBinary
        $env:WINDOWS_SIGNING_METADATA_PATH = $script:SyntheticBinary
        Mock Assert-WindowsSignature { }
        { Invoke-WindowsSigning -FilePath $script:SyntheticBinary } | Should -Throw '*SignTool failed with exit code 1*'
        Should -Invoke Assert-WindowsSignature -Times 0
    }

    It 'rejects SignTool warnings instead of treating them as successful signing' {
        $tool = Join-Path $TestDrive 'synthetic-signtool.ps1'
        Set-Content -LiteralPath $tool -Value '$global:LASTEXITCODE = 2'
        $env:WINDOWS_SIGNTOOL_PATH = $tool
        $env:WINDOWS_SIGNING_DLIB_PATH = $script:SyntheticBinary
        $env:WINDOWS_SIGNING_METADATA_PATH = $script:SyntheticBinary
        { Invoke-WindowsSigning -FilePath $script:SyntheticBinary } | Should -Throw '*SignTool failed with exit code 2*'
    }

    It 'propagates a verification failure after successful signing' {
        $tool = Join-Path $TestDrive 'synthetic-signtool.ps1'
        Set-Content -LiteralPath $tool -Value '$global:LASTEXITCODE = if ($args[0] -eq "sign") { 0 } else { 1 }'
        $env:WINDOWS_SIGNTOOL_PATH = $tool
        $env:WINDOWS_SIGNING_DLIB_PATH = $script:SyntheticBinary
        $env:WINDOWS_SIGNING_METADATA_PATH = $script:SyntheticBinary
        { Invoke-WindowsSigning -FilePath $script:SyntheticBinary } | Should -Throw '*SignTool verification failed*'
    }

    It 'passes paths with spaces as single arguments and verifies after signing' {
        $tool = Join-Path $TestDrive 'synthetic-signtool.ps1'
        Set-Content -LiteralPath $tool -Value '$global:SigningArguments += ,$args; $global:LASTEXITCODE = 0'
        $global:SigningArguments = @()
        $env:WINDOWS_SIGNTOOL_PATH = $tool
        $env:WINDOWS_SIGNING_DLIB_PATH = $script:SyntheticBinary
        $env:WINDOWS_SIGNING_METADATA_PATH = $script:SyntheticBinary
        Mock Assert-WindowsSignature { }
        Invoke-WindowsSigning -FilePath $script:SyntheticBinary
        $global:SigningArguments.Count | Should -Be 2
        $global:SigningArguments[0][0] | Should -Be 'sign'
        $global:SigningArguments[0][-1] | Should -Be $script:SyntheticBinary
        $global:SigningArguments[0] | Should -Contain 'SHA256'
        $global:SigningArguments[0] | Should -Contain 'http://timestamp.acs.microsoft.com'
        $global:SigningArguments[1][0] | Should -Be 'verify'
        $global:SigningArguments[1][-1] | Should -Be $script:SyntheticBinary
        Should -Invoke Assert-WindowsSignature -Times 1 -Exactly
    }
}

Describe 'Windows signing setup' {
    It 'configures only Azure CLI authentication and uses an argument-safe Tauri hook' {
        $values = Initialize-WindowsSigningConfiguration -Directory $TestDrive -Endpoint 'https://eus.codesigning.azure.net' `
            -AccountName 'syntheticaccount' -CertificateProfileName 'syntheticprofile'
        $metadata = Get-Content -LiteralPath $values.WINDOWS_SIGNING_METADATA_PATH -Raw | ConvertFrom-Json
        $metadata.CodeSigningAccountName | Should -Be 'syntheticaccount'
        $metadata.CertificateProfileName | Should -Be 'syntheticprofile'
        $metadata.ExcludeCredentials | Should -Contain 'EnvironmentCredential'
        $metadata.ExcludeCredentials | Should -Contain 'InteractiveBrowserCredential'
        $metadata.ExcludeCredentials | Should -Not -Contain 'AzureCliCredential'
        $config = Get-Content -LiteralPath $values.TAURI_SIGNING_CONFIG -Raw | ConvertFrom-Json
        $config.bundle.windows.signCommand.args[-1] | Should -Be '%1'
        $config.bundle.windows.signCommand.args[-2] | Should -Be (Join-Path $PSScriptRoot 'windows-signing.ps1')
        [IO.Path]::IsPathRooted($config.bundle.windows.signCommand.cmd) | Should -BeTrue
    }

    It 'rejects a non-Azure or non-HTTPS endpoint' -ForEach @(
        @{ Endpoint = 'http://eus.codesigning.azure.net' }
        @{ Endpoint = 'https://synthetic.example.org' }
    ) {
        { Initialize-WindowsSigningConfiguration -Directory $TestDrive -Endpoint $Endpoint `
                -AccountName 'syntheticaccount' -CertificateProfileName 'syntheticprofile' } |
            Should -Throw '*HTTPS Azure code-signing endpoint*'
    }

    It 'does not extract a dependency when its checksum is wrong' {
        Mock Invoke-WebRequest { Set-Content -LiteralPath $OutFile -Value 'synthetic archive' }
        Mock Expand-Archive { }
        { Expand-VerifiedSigningDownload -Uri 'https://synthetic.example.org/package.zip' `
                -Destination (Join-Path $TestDrive 'package') -Hash ('0' * 64) } |
            Should -Throw '*Signing dependency checksum failed*'
        Should -Invoke Expand-Archive -Times 0
    }
}
