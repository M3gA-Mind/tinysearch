$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $root

$target = ((rustc -vV | Select-String '^host: ').ToString().Split(':', 2)[1]).Trim()
if ($LASTEXITCODE -ne 0) { throw 'rustc -vV failed' }
$metadata = cargo metadata --manifest-path Cargo.toml --format-version 1 --no-deps | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$version = ($metadata.packages | Where-Object name -eq 'tinysearch' | Select-Object -First 1).version
if (-not $version) { throw 'tinysearch package version was not found in workspace metadata' }

$output = 'target/module-e2e'
& scripts/build-module.ps1 -Crate tinysearch -Version $version -Target $target -HostKey ci -OutputDir $output
if ($LASTEXITCODE -ne 0) { throw 'tinysearch module archive build failed' }
& scripts/verify-module.ps1 -Archive "$output/tinysearch-$version-ci.zip"
if ($LASTEXITCODE -ne 0) { throw 'tinysearch module E2E verification failed' }
