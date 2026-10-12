$ErrorActionPreference = 'Stop'
$script = Join-Path $PSScriptRoot 'test-distro.sh'
$previousMsys = $env:MSYS
try {
    # Git for Windows otherwise expands the distro script's default `[]`
    # argument while converting argv, and bash.exe can crash before running it.
    $env:MSYS = 'noglob'
    & bash $script @args
    $exitCode = $LASTEXITCODE
}
finally {
    $env:MSYS = $previousMsys
}
exit $exitCode
