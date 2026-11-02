# This script runs only on a disposable GitHub-hosted Windows x64 runner.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_OS -ne 'Windows' -or $env:RUNNER_ARCH -ne 'X64') {
    throw 'WSL provisioning requires a disposable GitHub-hosted Windows x64 runner.'
}

$name = 'AntiburnAcceptance-' + [guid]::NewGuid().ToString('N')
$directory = Join-Path $env:RUNNER_TEMP $name
$image = Join-Path $directory 'ubuntu-base.tar.gz'
$imported = $false
New-Item -ItemType Directory -Path $directory | Out-Null
try {
    $url = 'https://cdimage.ubuntu.com/ubuntu-base/releases/24.04.5/release/ubuntu-base-24.04.5-base-amd64.tar.gz'
    Invoke-WebRequest -Uri $url -OutFile $image -TimeoutSec 120
    $expected = 'e77b6f10c2590cef872b33ee9f635a0e3fd1f57fb074c0e52b5c7f56147a0c86'
    if ((Get-FileHash -Algorithm SHA256 $image).Hash.ToLowerInvariant() -ne $expected) {
        throw 'The isolated Ubuntu image did not match its pinned checksum.'
    }
    & wsl.exe --import $name (Join-Path $directory 'distribution') $image --version 2
    if ($LASTEXITCODE -ne 0) { throw 'WSL 2 import failed; required acceptance did not run.' }
    $imported = $true

    # Ubuntu base has no CA certificates. Bootstrap with signed archive metadata.
    # The snapshot requires HTTPS and pins the remaining packages.
    $setup = @'
set -eu
rm -f /etc/apt/sources.list.d/ubuntu.sources
cat > /etc/apt/sources.list <<'SOURCES'
deb http://archive.ubuntu.com/ubuntu noble main
SOURCES
export DEBIAN_FRONTEND=noninteractive
apt-get -o Acquire::Retries=2 -o APT::Update::Error-Mode=any update
apt-get install --yes --no-install-recommends ca-certificates=20240203
cat > /etc/apt/sources.list <<'SOURCES'
deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20261020T000000Z noble main
deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20261020T000000Z noble-updates main
deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20261020T000000Z noble-security main
SOURCES
apt-get -o Acquire::Retries=2 -o APT::Update::Error-Mode=any update
apt-get install --yes --no-install-recommends curl=8.5.0-2ubuntu10.16 python3=3.12.3-0ubuntu2.1
useradd --create-home --shell /bin/sh antiburn
printf '[user]\ndefault=antiburn\n' > /etc/wsl.conf
dpkg-query --show ca-certificates curl python3
'@
    $start = [System.Diagnostics.ProcessStartInfo]::new('wsl.exe')
    foreach ($argument in @('--distribution', $name, '--user', 'root', '--exec', 'sh', '-eu')) {
        $start.ArgumentList.Add($argument)
    }
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $process = [System.Diagnostics.Process]::Start($start)
    $process.StandardInput.Write($setup.Replace("`r", '') + "`n")
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(300000)) {
        $process.Kill()
        throw 'Isolated WSL package setup exceeded five minutes.'
    }
    if ($process.ExitCode -ne 0) { throw 'Isolated WSL package setup failed.' }

    & wsl.exe --terminate $name
    if ($LASTEXITCODE -ne 0) { throw 'Could not restart the owned acceptance distro.' }
    $defaultUser = (& wsl.exe --distribution $name --exec id -un | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $defaultUser -ne 'antiburn') {
        throw 'The owned acceptance distro did not apply its non-root default user.'
    }

    $env:ANTIBURN_WSL_ACCEPTANCE_DISTRIBUTION = $name
    & cargo nextest run --locked --manifest-path crates/antiburn-local/Cargo.toml `
        --test opencode_wsl_acceptance --run-ignored ignored-only --success-output immediate `
        -E 'test(production_bridge_reads_registered_wsl_service_without_starting_opencode)'
    if ($LASTEXITCODE -ne 0) { throw 'Required real WSL bridge acceptance failed.' }
} finally {
    Remove-Item Env:ANTIBURN_WSL_ACCEPTANCE_DISTRIBUTION -ErrorAction SilentlyContinue
    if ($imported) {
        & wsl.exe --unregister $name
        if ($LASTEXITCODE -ne 0) { throw "Failed to remove the owned acceptance distro $name." }
    }
    if (Test-Path $directory) { Remove-Item -Recurse -Force $directory }
}
