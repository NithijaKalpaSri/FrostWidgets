# Builds the app, then the installer with the app compressed inside it.
# Output: dist\FrostWidgets-Setup-<version>.exe
#
# Default: the normal edition (per-user install, no admin prompt, unsigned). It works
# on any PC; the taskbar pill hides while Start or search is open.
# -UIAccess: the edition for this PC only (Program Files, admin prompt, signed with the
# local certificate from uiaccess-setup.ps1), whose pill stays visible over Start.
param([switch]$UIAccess)
$ErrorActionPreference = "Continue"  # cargo writes progress to stderr
Set-Location $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path

Remove-Item Env:FROST_PAYLOAD -ErrorAction SilentlyContinue
if ($UIAccess) { $env:FROST_UIACCESS = "1" } else { Remove-Item Env:FROST_UIACCESS -ErrorAction SilentlyContinue }
cargo build --release --bin frost
if ($LASTEXITCODE -ne 0) { exit 1 }

if ($UIAccess) {
    # Its UI Access manifest only works for a signed exe.
    $cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object Subject -eq "CN=Frost Widgets Local Signing" | Select-Object -First 1
    if (-not $cert) { "No signing certificate: run uiaccess-setup.ps1 as admin first."; exit 1 }
    $sig = Set-AuthenticodeSignature -FilePath "target\release\frost.exe" -Certificate $cert -HashAlgorithm SHA256
    if ($sig.Status -ne "Valid") { "Signing failed: $($sig.StatusMessage)"; exit 1 }
}

$env:FROST_PAYLOAD = (Resolve-Path "target\release\frost.exe").Path
cargo build --release --bin frost-setup
if ($LASTEXITCODE -ne 0) { exit 1 }
Remove-Item Env:FROST_PAYLOAD
Remove-Item Env:FROST_UIACCESS -ErrorAction SilentlyContinue

$ver = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force dist | Out-Null
$name = if ($UIAccess) { "FrostWidgets-Setup-$ver-UIAccess.exe" } else { "FrostWidgets-Setup-$ver.exe" }
$out = "dist\$name"
Copy-Item "target\release\frost-setup.exe" $out -Force
"{0}  {1:N0} KB" -f $out, ((Get-Item $out).Length / 1KB)
