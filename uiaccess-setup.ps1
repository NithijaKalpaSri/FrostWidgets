# Frost Widgets: UI Access test setup (run in an ADMIN PowerShell).
# 1. Makes a code-signing certificate for this PC only (private key can't be exported).
# 2. Trusts it machine-wide (Trusted Root + Trusted Publishers).
# 3. Signs target\release\frost.exe with it.
# 4. Copies the signed exe to C:\Program Files\Frost Widgets UIAccess Test\ with a
#    portable config set to the Taskbar style, so your normal install is untouched.
# Undo everything with: .\uiaccess-setup.ps1 -Remove
param([switch]$Remove)
$ErrorActionPreference = "Stop"
$subject = "CN=Frost Widgets Local Signing"
$dest = "C:\Program Files\Frost Widgets UIAccess Test"

if ($Remove) {
    Get-Process frost -ErrorAction SilentlyContinue | Where-Object Path -like "$dest*" | Stop-Process -Force
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($store in "Cert:\LocalMachine\Root", "Cert:\LocalMachine\TrustedPublisher", "Cert:\CurrentUser\My") {
        Get-ChildItem $store | Where-Object Subject -eq $subject | Remove-Item -Force
    }
    "Removed the test folder and the certificate."
    return
}

$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object Subject -eq $subject | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -CertStoreLocation Cert:\CurrentUser\My -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddYears(5)
}
$cer = Join-Path $env:TEMP "frost-signing.cer"
Export-Certificate -Cert $cert -FilePath $cer | Out-Null
Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
Remove-Item $cer

$exe = Join-Path $PSScriptRoot "target\release\frost.exe"
$sig = Set-AuthenticodeSignature -FilePath $exe -Certificate $cert -HashAlgorithm SHA256
"Signature: $($sig.Status)"

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item $exe "$dest\frost.exe" -Force
$cfg = (Get-Content "$env:APPDATA\FrostWidgets\config.toml" -Raw) -replace 'media_style = "\w+"', 'media_style = "taskbar"'
Set-Content "$dest\config.toml" $cfg -Encoding utf8
"Ready: $dest\frost.exe"
