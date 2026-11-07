# ---------------------------------------------------------------------------
# VERIFICADOR de integridad de la WALLET PORTABLE (Fase 3d).
#
# Uso: powershell -File verificar_portable.ps1   (o doble derecho > RunWith)
# Compara el hash SHA-256 REAL de cada archivo contra SHA256SUMS.txt.
# Exit 0 = paquete intacto; exit 1 = contaminado/corrupto o incompleto.
# ---------------------------------------------------------------------------
$ErrorActionPreference = "Stop"
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$sums = Join-Path $dir "SHA256SUMS.txt"

if (!(Test-Path $sums)) {
    Write-Host "ERROR: No hay SHA256SUMS.txt en $dir" -ForegroundColor Red
    Write-Host "       La carpeta no es un paquete portable completo." -ForegroundColor Red
    exit 1
}

Write-Host "Verificando integridad del paquete portable: $dir"
$ok = $true
foreach ($linea in (Get-Content $sums)) {
    if (![string]::IsNullOrWhiteSpace($linea)) {
        $partes = $linea -split "\s+", 2
        $hash = $partes[0]
        $nombre = $partes[1].Trim()
        $path = Join-Path $dir $nombre
        if (Test-Path $path) {
            $real = (Get-FileHash -Algorithm SHA256 -Path $path).Hash
            if ($real -eq $hash) {
                Write-Host "  OK   $nombre" -ForegroundColor Green
            } else {
                Write-Host "  MAL  $nombre — el hash difiere (esperado $hash, real $real)" -ForegroundColor Red
                $ok = $false
            }
        } else {
            Write-Host "  FALTA $nombre" -ForegroundColor Red
            $ok = $false
        }
    }
}

if ($ok) {
    Write-Host "PAQUETE INTACTO — todos los hashes coinciden." -ForegroundColor Green
    exit 0
}
Write-Host "PAQUETE CONTAMINADO O CORRUPTO — NO distribuir, NO ejecutar." -ForegroundColor Red
exit 1
