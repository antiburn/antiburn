BeforeAll {
    $script:RepositoryRoot = Split-Path -Parent $PSScriptRoot
    $script:InstallerPath = Join-Path $script:RepositoryRoot 'install.ps1'
    . $script:InstallerPath
}

Describe 'install.ps1' {
    Context 'Windows architecture' {
        It 'selects x64 for an AMD64 processor' {
            Mock Get-CimInstance { [PSCustomObject]@{ Architecture = 9 } }
            Get-WindowsArchitecture | Should -Be 'x64'
        }

        It 'selects ARM64 even when PowerShell runs under x64 emulation' {
            Mock Get-CimInstance { [PSCustomObject]@{ Architecture = 12 } }
            $previous = $env:PROCESSOR_ARCHITECTURE
            try {
                $env:PROCESSOR_ARCHITECTURE = 'AMD64'
                Get-WindowsArchitecture | Should -Be 'arm64'
            }
            finally {
                $env:PROCESSOR_ARCHITECTURE = $previous
            }
        }

        It 'rejects unsupported processor architecture <Architecture>' -ForEach @(0, 5, 6) {
            Mock Get-CimInstance { [PSCustomObject]@{ Architecture = $Architecture } }
            { Get-WindowsArchitecture } | Should -Throw '*Unsupported Windows processor architecture*'
        }

        It 'rejects an empty processor query' {
            Mock Get-CimInstance { }
            { Get-WindowsArchitecture } | Should -Throw '*Unsupported Windows processor architecture*'
        }
    }

    Context 'Architecture-specific installation' {
        BeforeEach {
            Mock Write-InstallerBanner { }
            Mock Write-InstallerInfo { }
            Mock Write-Warning { }
            Mock Get-AntiburnRelease {
                [PSCustomObject]@{ Version = '1.2.3'; Tag = 'antiburn-v1.2.3' }
            }
            Mock Invoke-InstallerDownload {
                if ($Uri.AbsolutePath.EndsWith('/SHA256SUMS')) {
                    $hash = (Get-FileHash -LiteralPath $script:FixtureInstaller -Algorithm SHA256).Hash
                    Set-Content -LiteralPath $OutFile -Value "$hash  $script:ExpectedAsset"
                }
                else {
                    Copy-Item -LiteralPath $script:FixtureInstaller -Destination $OutFile
                }
            }
            Mock Start-Process { [PSCustomObject]@{ ExitCode = 0 } }
            Mock Test-Path { $true } -ParameterFilter {
                $LiteralPath -eq (Join-Path $env:LOCALAPPDATA 'antiburn\antiburn.exe')
            }
            $script:FixtureInstaller = Join-Path $TestDrive 'synthetic-installer.exe'
            Set-Content -LiteralPath $script:FixtureInstaller -Value 'synthetic installer'
            $script:PreviousVersion = $env:ANTIBURN_VERSION
            $script:PreviousAttestation = $env:ANTIBURN_VERIFY_ATTESTATION
            $env:ANTIBURN_VERSION = ''
            $env:ANTIBURN_VERIFY_ATTESTATION = ''
        }

        AfterEach {
            $env:ANTIBURN_VERSION = $script:PreviousVersion
            $env:ANTIBURN_VERIFY_ATTESTATION = $script:PreviousAttestation
        }

        It 'downloads and verifies the <Label> installer before starting it' -ForEach @(
            @{ Architecture = 9; Label = 'x64' }
            @{ Architecture = 12; Label = 'arm64' }
        ) {
            Mock Get-CimInstance { [PSCustomObject]@{ Architecture = $Architecture } }
            $script:ExpectedAsset = "antiburn_1.2.3_${Label}-setup.exe"

            Invoke-AntiburnInstall -RequestedVersion '1.2.3'

            Should -Invoke Invoke-InstallerDownload -Times 1 -Exactly -ParameterFilter {
                $Uri.AbsoluteUri -eq "https://github.com/antiburn/antiburn/releases/download/antiburn-v1.2.3/$script:ExpectedAsset"
            }
            Should -Invoke Start-Process -Times 1 -Exactly -ParameterFilter {
                (Split-Path -Leaf $FilePath) -eq $script:ExpectedAsset -and
                $ArgumentList -eq '/P /R' -and $Wait -and $PassThru
            }
        }

        It 'does not start the ARM64 installer when checksum verification fails' {
            Mock Get-CimInstance { [PSCustomObject]@{ Architecture = 12 } }
            $script:ExpectedAsset = 'antiburn_1.2.3_arm64-setup.exe'
            Mock Invoke-InstallerDownload {
                Set-Content -LiteralPath $OutFile -Value "$('a' * 64)  $script:ExpectedAsset"
            }

            { Invoke-AntiburnInstall -RequestedVersion '1.2.3' } |
                Should -Throw '*Checksum verification failed*'
            Should -Invoke Start-Process -Times 0 -Exactly
        }
    }

    It 'selects one exact checksum entry' {
        $checksums = Join-Path $TestDrive 'SHA256SUMS'
        $hash = 'a' * 64
        Set-Content -LiteralPath $checksums -Value "$hash  antiburn_1.2.3_x64-setup.exe"

        Get-ExpectedChecksum -ChecksumFile $checksums -AssetName 'antiburn_1.2.3_x64-setup.exe' |
            Should -Be $hash
    }

    It 'rejects duplicate checksum entries' {
        $checksums = Join-Path $TestDrive 'SHA256SUMS'
        $hash = 'a' * 64
        Set-Content -LiteralPath $checksums -Value @(
            "$hash  antiburn_1.2.3_x64-setup.exe"
            "$hash  antiburn_1.2.3_x64-setup.exe"
        )

        {
            Get-ExpectedChecksum -ChecksumFile $checksums -AssetName 'antiburn_1.2.3_x64-setup.exe'
        } | Should -Throw '*exactly one valid entry*'
    }

    It 'verifies the installer SHA-256 value' {
        $installerFile = Join-Path $TestDrive 'antiburn_1.2.3_x64-setup.exe'
        $checksums = Join-Path $TestDrive 'SHA256SUMS'
        Set-Content -LiteralPath $installerFile -Value 'synthetic installer'
        $hash = (Get-FileHash -LiteralPath $installerFile -Algorithm SHA256).Hash.ToLowerInvariant()
        Set-Content -LiteralPath $checksums -Value "$hash  antiburn_1.2.3_x64-setup.exe"

        { Assert-InstallerIntegrity -InstallerPath $installerFile -ChecksumFile $checksums } |
            Should -Not -Throw
    }

    It 'rejects a checksum mismatch' {
        $installerFile = Join-Path $TestDrive 'antiburn_1.2.3_x64-setup.exe'
        $checksums = Join-Path $TestDrive 'SHA256SUMS'
        Set-Content -LiteralPath $installerFile -Value 'synthetic installer'
        Set-Content -LiteralPath $checksums -Value "$('a' * 64)  antiburn_1.2.3_x64-setup.exe"

        { Assert-InstallerIntegrity -InstallerPath $installerFile -ChecksumFile $checksums } |
            Should -Throw '*Checksum verification failed*'
    }

    It 'resolves the latest release from the GitHub web redirect' {
        Mock Invoke-WebRequest {
            [PSCustomObject]@{
                BaseResponse = [PSCustomObject]@{
                    ResponseUri = [uri] 'https://github.com/antiburn/antiburn/releases/tag/antiburn-v1.2.3'
                }
            }
        }

        $release = Get-AntiburnRelease
        $release.Version | Should -Be '1.2.3'
        $release.Tag | Should -Be 'antiburn-v1.2.3'
        Should -Invoke Invoke-WebRequest -Times 1 -Exactly -ParameterFilter {
            $Uri -eq 'https://github.com/antiburn/antiburn/releases/latest' -and $Method -eq 'Head'
        }
    }

    It 'reads the redirect target from a PowerShell 7 response' {
        Mock Invoke-WebRequest {
            [PSCustomObject]@{
                BaseResponse = [PSCustomObject]@{
                    RequestMessage = [PSCustomObject]@{
                        RequestUri = [uri] 'https://github.com/antiburn/antiburn/releases/tag/antiburn-v1.2.3'
                    }
                }
            }
        }

        (Get-AntiburnRelease).Version | Should -Be '1.2.3'
    }

    It 'does not call the GitHub REST API' {
        $source = Get-Content -LiteralPath $script:InstallerPath -Raw
        $source | Should -Not -Match 'api\.github\.com'
        $source | Should -Not -Match 'Invoke-RestMethod'
    }

    It 'rejects a redirect that is not a release tag' {
        Mock Invoke-WebRequest {
            [PSCustomObject]@{
                BaseResponse = [PSCustomObject]@{
                    ResponseUri = [uri] 'https://github.com/antiburn/antiburn/releases'
                }
            }
        }

        { Get-AntiburnRelease } | Should -Throw '*unexpected release URL*'
    }

    It 'rejects a release tag from another product' {
        Mock Invoke-WebRequest {
            [PSCustomObject]@{
                BaseResponse = [PSCustomObject]@{
                    ResponseUri = [uri] 'https://github.com/antiburn/antiburn/releases/tag/antiburn-local-v0.1.6'
                }
            }
        }

        { Get-AntiburnRelease } | Should -Throw '*invalid release tag*'
    }

    It 'skips the lookup when a version is requested' {
        Mock Invoke-WebRequest { throw 'The request must not run.' }

        (Get-AntiburnRelease -RequestedVersion '1.2.3').Tag | Should -Be 'antiburn-v1.2.3'
        Should -Invoke Invoke-WebRequest -Times 0 -Exactly
    }

    It 'rejects a non-HTTPS download before making a request' {
        Mock Invoke-WebRequest { throw 'The request must not run.' }

        {
            Invoke-InstallerDownload -Uri 'http://example.test/installer.exe' -OutFile (Join-Path $TestDrive 'installer.exe')
        } | Should -Throw '*Refusing a non-HTTPS download*'
        Should -Invoke Invoke-WebRequest -Times 0 -Exactly
    }

    It 'rejects and removes a download redirected to HTTP' {
        $output = Join-Path $TestDrive 'installer.exe'
        Mock Invoke-WebRequest {
            [PSCustomObject]@{
                BaseResponse = [PSCustomObject]@{
                    ResponseUri = [uri] 'http://example.test/installer.exe'
                }
            }
        }

        {
            Invoke-InstallerDownload -Uri 'https://example.test/installer.exe' -OutFile $output
        } | Should -Throw '*redirected to http://*'
        Test-Path -LiteralPath $output | Should -BeFalse
    }

    It 'writes the response stream to the download path' {
        $output = Join-Path $TestDrive 'installer.exe'
        $bytes = [System.Text.Encoding]::UTF8.GetBytes('synthetic installer')
        Mock Invoke-WebRequest {
            [PSCustomObject]@{
                BaseResponse = [PSCustomObject]@{
                    ResponseUri = [uri] 'https://example.test/installer.exe'
                }
                RawContentStream = [System.IO.MemoryStream]::new($bytes)
            }
        }

        Invoke-InstallerDownload -Uri 'https://example.test/installer.exe' -OutFile $output
        [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($output)) |
            Should -Be 'synthetic installer'
    }

    It 'uses safe web parsing and the passive NSIS mode' {
        $source = Get-Content -LiteralPath $script:InstallerPath -Raw
        $source | Should -Match 'UseBasicParsing = \$true'
        $source | Should -Match "-ArgumentList '/P /R'"
        $source | Should -Not -Match 'Invoke-Expression'
    }

    It 'documents the current unsigned installer state' {
        $source = Get-Content -LiteralPath $script:InstallerPath -Raw
        $source | Should -Match 'not required to have an Authenticode signature yet'
        $source | Should -Match 'SmartScreen can warn'
    }

    It 'binds the version parameter when the script runs through Invoke-Expression' {
        # The documented Windows command pipes this file into Invoke-Expression.
        # PowerShell then runs the file as a script block and adds each param
        # attribute to a variable in the caller scope.
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $script:InstallerPath, [ref] $null, [ref] $null)
        $paramBlock = $ast.ParamBlock.Extent.Text
        $previous = $env:ANTIBURN_VERSION
        try {
            $env:ANTIBURN_VERSION = ''
            { $paramBlock | Invoke-Expression } | Should -Not -Throw
            $env:ANTIBURN_VERSION = '1.2.3'
            { $paramBlock | Invoke-Expression } | Should -Not -Throw
        }
        finally {
            $env:ANTIBURN_VERSION = $previous
        }
    }

    Context 'Resolve-RequestedVersion' {
        BeforeAll {
            $script:PreviousRequestedVersion = $env:ANTIBURN_VERSION
        }

        AfterAll {
            $env:ANTIBURN_VERSION = $script:PreviousRequestedVersion
        }

        It 'prefers the parameter over the environment variable' {
            $env:ANTIBURN_VERSION = '9.9.9'
            Resolve-RequestedVersion -Version '1.2.3' | Should -Be '1.2.3'
        }

        It 'reads the environment variable when the parameter is empty' {
            $env:ANTIBURN_VERSION = '1.2.3'
            Resolve-RequestedVersion -Version '' | Should -Be '1.2.3'
        }

        It 'returns nothing when neither source gives a version' {
            $env:ANTIBURN_VERSION = ''
            [string]::IsNullOrEmpty((Resolve-RequestedVersion -Version '')) | Should -BeTrue
        }

        It 'rejects a version that holds unsafe characters' {
            $env:ANTIBURN_VERSION = ''
            { Resolve-RequestedVersion -Version '1.2.3;calc' } |
                Should -Throw '*Invalid version*'
        }

        It 'rejects an unsafe version from the environment variable' {
            $env:ANTIBURN_VERSION = '1.2.3;calc'
            { Resolve-RequestedVersion -Version '' } | Should -Throw '*Invalid version*'
        }
    }
}
