# vendor-local-deps.ps1
# Regenera el directorio vendored local_deps/ para el build offline en Windows.
# Uso: powershell -ExecutionPolicy Bypass -File vendor-local-deps.ps1
# Requiere: cargo + Cargo.toml/Cargo.lock válidos. Ejecutar desde cualquier CWD.

$ErrorActionPreference = "Stop"

# ── 0. Banner ──
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " Ledgerita - Cargo Vendor (local_deps)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host ""

# ── 1. Resolver directorio del proyecto (ledgerita_core) ──
$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$CargoToml = Join-Path $ProjectRoot "Cargo.toml"
$CargoLock = Join-Path $ProjectRoot "Cargo.lock"
$CargoConfig = Join-Path $ProjectRoot ".cargo\config.toml"
$LocalDeps = Join-Path $ProjectRoot "local_deps"

if (-not (Test-Path -LiteralPath $CargoToml)) {
    Write-Host "ERROR: No se encontro Cargo.toml en $ProjectRoot" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path -LiteralPath $CargoLock)) {
    Write-Host "ERROR: No se encontro Cargo.lock en $ProjectRoot" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path -LiteralPath $CargoConfig)) {
    Write-Host "WARNING: No existe .cargo/config.toml. El build NO usara fuentes vendered." -ForegroundColor Yellow
}

Write-Host "Proyecto: $ProjectRoot" -ForegroundColor Green

# ── 2. Limpiar vendor previo ──
Write-Host "`n[1/3] Limpiando local_deps existente..." -ForegroundColor Yellow
if (Test-Path -LiteralPath $LocalDeps) {
    Remove-Item -LiteralPath $LocalDeps -Recurse -Force
}
# Limpiar artefacto residual de un vendor anterior (config.toml generado por cargo)
$VendorConfig = Join-Path $ProjectRoot "local_deps\config.toml"
if (Test-Path -LiteralPath $VendorConfig) {
    Remove-Item -LiteralPath $VendorConfig -Force
}
Write-Host "    OK" -ForegroundColor Green

# ── 3. Ejecutar cargo vendor ──
Write-Host "`n[2/3] Ejecutando cargo vendor local_deps..." -ForegroundColor Yellow
Set-Location -LiteralPath $ProjectRoot
cargo vendor local_deps
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: cargo vendor fallo." -ForegroundColor Red
    Write-Host "  Si es por rutas largas en Windows, desactiva el vendor:" -ForegroundColor Yellow
    Write-Host "    1. Renombra temporalmente .cargo\config.toml" -ForegroundColor Yellow
    Write-Host "    2. cargo build (compila directo de crates.io)" -ForegroundColor Yellow
    Write-Host "    3. Restaura .cargo\config.toml al terminar" -ForegroundColor Yellow
    exit 1
}
Write-Host "    OK" -ForegroundColor Green

# ── 4. Limpiar config.toml generado por cargo dentro de local_deps ──
# (el reemplazo de fuentes ya lo define nuestro .cargo/config.toml)
$VendorConfig = Join-Path $ProjectRoot "local_deps\config.toml"
if (Test-Path -LiteralPath $VendorConfig) {
    Remove-Item -LiteralPath $VendorConfig -Force
    Write-Host "    (eliminado local_deps\config.toml generado por cargo)" -ForegroundColor DarkGray
}

# ── 5. Verificar con check offline ──
Write-Host "`n[3/3] Verificando con cargo check --offline..." -ForegroundColor Yellow
cargo check --offline
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: cargo check --offline fallo. El vendor no es consistente." -ForegroundColor Red
    exit 1
}
Write-Host "    OK" -ForegroundColor Green

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " Vendor regenerado correctamente." -ForegroundColor Cyan
Write-Host " Build offline listo: cargo build --offline" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
