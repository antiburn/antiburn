BeforeAll {
    $script:RepositoryRoot = Split-Path -Parent $PSScriptRoot
    $script:InstallerPath = Join-Path $script:RepositoryRoot 'install.ps1'
    . $script:InstallerPath
}

Describe 'install.ps1' {
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
