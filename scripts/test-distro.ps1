$ErrorActionPreference = 'Stop'
$script = Join-Path $PSScriptRoot 'test-distro.sh'
& bash $script @args
$exitCode = $LASTEXITCODE
exit $exitCode
