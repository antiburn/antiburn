param([Parameter(Mandatory=$true)][string]$Binary)
$ErrorActionPreference = 'Stop'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$dumpbin = & $vswhere -latest -products '*' -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe' | Select-Object -First 1
if (-not $dumpbin) { throw 'Visual Studio dumpbin is required to audit the CLI payload.' }
& $dumpbin /dependents $Binary
if ($LASTEXITCODE -ne 0) { throw 'dumpbin failed to inspect native CLI dependencies.' }
