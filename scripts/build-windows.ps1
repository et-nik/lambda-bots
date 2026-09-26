# Builds lambdabots_mm.dll (Win32, MSVC, static CRT) and checks exports and imports with dumpbin.
# Run from a Visual Studio x86 developer prompt (dumpbin must be on PATH).
param([string]$Preset = "release-windows-x86")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    rustup target add i686-pc-windows-msvc
    cmake --preset $Preset
    if ($LASTEXITCODE -ne 0) { throw "cmake configure failed" }
    cmake --build --preset $Preset
    if ($LASTEXITCODE -ne 0) { throw "cmake build failed" }

    $dll = Join-Path $root "build/$Preset/Release/lambdabots_mm.dll"
    $names = dumpbin /nologo /exports $dll |
        Select-String -Pattern '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]{8}\s+(\S+)' |
        ForEach-Object { $_.Matches[0].Groups[1].Value } | Sort-Object
    $expected = @("GiveFnptrsToDll", "Meta_Attach", "Meta_Detach", "Meta_Init", "Meta_Query")
    if (Compare-Object @($names) $expected) {
        throw "exports are [$($names -join ' ')], expected [$($expected -join ' ')]"
    }
    $deps = dumpbin /nologo /dependents $dll |
        Select-String -Pattern '^\s+(\S+\.dll)\s*$' |
        ForEach-Object { $_.Matches[0].Groups[1].Value.ToLower() }
    if ($deps | Where-Object { $_ -match 'msvcp|vcruntime|api-ms-win-crt' }) {
        throw "dynamic MSVC runtime imported: $($deps -join ' ')"
    }
    $machine = dumpbin /nologo /headers $dll | Select-String -Pattern 'machine \(x86\)'
    if (-not $machine) { throw "not an x86 image" }
    Write-Host "ok: $dll exports [$($names -join ' ')], imports [$($deps -join ' ')]"
} finally {
    Pop-Location
}
