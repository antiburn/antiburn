# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
}
