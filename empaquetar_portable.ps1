# ---------------------------------------------------------------------------
# Empaqueta la LEDGERITA WALLET PORTABLE (Fase 3d).
#
# Resultado: dist\LedgeritaWalletPortatil\ con
#   - ledgerita_wallet.exe        (el binario recién compilado)
#   - portable.flag               (marcador: datos en ledgerita-data/ local)
#   - ledgerita-data\             (carpeta vacía para datos de las wallets)
#   - verificar_portable.ps1      (verificador de integridad)
#   - SHA256SUMS.txt              (hashes de los tres archivos de arriba)
#
# Anti-contaminación: el verificador compara el hash REAL de cada archivo
# contra el manifest. Correrlo ANTES de distribuir el paquete (debe salir
# "intacto") y de nuevo después de copiarlo a USB/otra máquina — si difiere,
# el archivo llegó corrupto o modificado.
# ---------------------------------------------------------------------------
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

Write-Host "== 1/4 Compilando la wallet (release) =="
cargo build --release --manifest-path ledgerita_wallet\Cargo.toml
if ($LASTEXITCODE -ne 0) { throw "cargo build fallo" }

Write-Host "== 2/4 Armando el paquete dist\LedgeritaWalletPortatil =="
$dist = Join-Path $root "dist\LedgeritaWalletPortatil"
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue | Out-Null
New-Item -ItemType Directory -Force -Path $dist | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $dist "ledgerita-data") | Out-Null
New-Item -ItemType File -Force -Path (Join-Path $dist "portable.flag") | Out-Null
Copy-Item "ledgerita_wallet\target\release\ledgerita_wallet.exe" $dist -Force
Copy-Item "$root\scripts\verificar_portable.ps1" $dist -Force

Write-Host "== 3/4 Generando SHA256SUMS.txt =="
$files = @("ledgerita_wallet.exe", "portable.flag", "verificar_portable.ps1") |
    ForEach-Object { Join-Path $dist $_ }
($files | ForEach-Object { Get-FileHash -Algorithm SHA256 -Path $_ } | ForEach-Object {
    "$($_.Hash.ToUpper())  $(Split-Path $_.Path -Leaf)"
}) | Set-Content -Path (Join-Path $dist "SHA256SUMS.txt") -Encoding ASCII
Get-Content (Join-Path $dist "SHA256SUMS.txt") | ForEach-Object { "  $_" }

Write-Host "== 4/4 Auto-verificación del paquete recién empaquetado =="
powershell -ExecutionPolicy Bypass -File (Join-Path $dist "verificar_portable.ps1")
if ($LASTEXITCODE -ne 0) { throw "El paquete recien empaquetado NO verifico" }

Write-Host ""
Write-Host "🎉 Paquete portable listo en dist\LedgeritaWalletPortatil"
Write-Host "   Para correrlo: doble click a ledgerita_wallet.exe (datos en ledgerita-data\ local)."
Write-Host "   Para verificarlo en cualquier destino: powershell -File verificar_portable.ps1"
