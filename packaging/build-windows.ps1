# Builds a portable zip for Windows x64 in dist\.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$name = "Sunburst-Disk-Analyser-$version-windows-x64"
$out = Join-Path "dist" $name
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "target\release\sunburst-disk-analyser.exe" (Join-Path $out "Sunburst Disk Analyser.exe")
Copy-Item README.md, LICENSE $out
Compress-Archive -Path $out -DestinationPath "dist\$name.zip" -Force
Remove-Item -Recurse -Force $out
Write-Host "Built dist\$name.zip"
