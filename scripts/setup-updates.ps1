# One-time setup for automatic updates (run from the app folder):
#   powershell -ExecutionPolicy Bypass -File scripts\setup-updates.ps1
#
# 1. Creates the update signing key pair in your user folder (never inside the project).
# 2. Writes the public key into crates/gitcraft-core/src/updates.rs, so installed copies only accept
#    updates you signed.
# 3. Stores the private key and its password as GitHub Actions secrets (needs the GitHub CLI, `gh`),
#    or tells you exactly what to paste into GitHub if `gh` isn't installed.
param(
    [string]$Repo = "MystifiedSky/GitCraft",
    [string]$KeyPath = (Join-Path $env:USERPROFILE ".tauri\gitcraft-updater.key")
)
$ErrorActionPreference = "Stop"
$app = Split-Path -Parent $PSScriptRoot
if (-not (Get-Command cargo-packager -ErrorAction SilentlyContinue)) { throw "Install the packager first: cargo install cargo-packager --locked --version 0.11.8" }

$password = ""
if (Test-Path $KeyPath) {
    Write-Host "Using the existing key at $KeyPath"
    $secure = Read-Host "Password of that key (press Enter if it has none)" -AsSecureString
} else {
    New-Item -ItemType Directory -Force (Split-Path $KeyPath) | Out-Null
    $secure = Read-Host "Choose a password for the new signing key" -AsSecureString
}
$password = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
if (-not (Test-Path $KeyPath)) {
    & cargo packager signer generate --ci --password $password --path $KeyPath
    if ($LASTEXITCODE -ne 0) { throw "Could not create the key." }
}
$pub = (Get-Content "$KeyPath.pub" -Raw).Trim()

# Put the public key in updates.rs.
$updates = Join-Path $app "crates\gitcraft-core\src\updates.rs"
$text = Get-Content $updates -Raw
$text = $text -replace 'pub const PUBLIC_KEY: &str = "[^"]*";', "pub const PUBLIC_KEY: &str = `"$pub`";"
Set-Content -Path $updates -Value $text -NoNewline
Write-Host "Public key written to crates\gitcraft-core\src\updates.rs. Commit that change."

$gh = Get-Command gh -ErrorAction SilentlyContinue
if ($gh) {
    Get-Content $KeyPath -Raw | gh secret set TAURI_SIGNING_PRIVATE_KEY --repo $Repo
    $password | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo $Repo
    Write-Host "Secrets TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD saved on $Repo."
} else {
    Write-Host ""
    Write-Host "GitHub CLI not found. On https://github.com/$Repo/settings/secrets/actions add two secrets:"
    Write-Host "  TAURI_SIGNING_PRIVATE_KEY           = the whole contents of $KeyPath"
    Write-Host "  TAURI_SIGNING_PRIVATE_KEY_PASSWORD  = the password you just typed"
}
Write-Host ""
Write-Host "IMPORTANT: back up $KeyPath and its password somewhere safe (a password manager)."
Write-Host "If they are lost, copies already installed can never be updated automatically again."
