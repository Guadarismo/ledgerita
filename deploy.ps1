# ─────────────────────────────────────────────────────
# Ledgerita Deploy Script
# Empaqueta, sube, compila e instala systemd en servidores
# ─────────────────────────────────────────────────────

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$TarFile = Join-Path $ProjectRoot "ledgerita_core.tar"
$SshKey = "$env:USERPROFILE\.ssh\id_ed25519_ledgerita"

# Servidores de producción
$Servers = @(
    @{ Name = "kettle03"; Ip = "192.168.68.61"; Port = "36565" }
    @{ Name = "kettle02"; Ip = "192.168.68.64"; Port = "0" }
)

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Ledgerita Deploy" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ── 1. Compilar y testear (local) ──
Write-Host "`n[1/4] Compilando y testeando localmente..." -ForegroundColor Yellow
Set-Location (Join-Path $ProjectRoot "ledgerita_core")
cargo test --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Tests fallaron. Abortando deploy." -ForegroundColor Red
    exit 1
}
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Build falló. Abortando deploy." -ForegroundColor Red
    exit 1
}
Write-Host "✅ Build y tests OK." -ForegroundColor Green

# ── 2. Crear tar (excluyendo target y local_deps) ──
Write-Host "`n[2/4] Creando tarball..." -ForegroundColor Yellow
Remove-Item -LiteralPath $TarFile -ErrorAction SilentlyContinue
Set-Location $ProjectRoot
tar cf $TarFile -C $ProjectRoot --exclude="ledgerita_core/target" --exclude="ledgerita_core/local_deps" ledgerita_core
if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Error creando tarball. Abortando deploy." -ForegroundColor Red
    exit 1
}
Write-Host "✅ Tarball creado: $TarFile" -ForegroundColor Green

# ── 3. Subir a servidores ──
Write-Host "`n[3/4] Subiendo a servidores..." -ForegroundColor Yellow

function Send-File {
    param([string]$ServerName, [string]$LocalFile)

    # Intento 1: SCP directo
    scp -i "$SshKey" -o ConnectTimeout=30 $LocalFile "${ServerName}:~/"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "    ⚠️  SCP falló, reintentando con SFTP..." -ForegroundColor Yellow
        $batch = Join-Path $env:TEMP "upload_sftp.txt"
        Set-Content -Path $batch -Value "put $LocalFile"
        try {
            sftp -b $batch -i "$SshKey" "${ServerName}:"
            if ($LASTEXITCODE -ne 0) {
                Write-Host "    ❌ SFTP también falló" -ForegroundColor Red
                return $false
            }
            Write-Host "    ✅ Subido vía SFTP" -ForegroundColor Green
        } finally {
            Remove-Item -LiteralPath $batch -ErrorAction SilentlyContinue
        }
    } else {
        Write-Host "    ✅ Subido vía SCP" -ForegroundColor Green
    }

    # Verificar integridad con SHA-256
    $localHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $LocalFile).Hash
    $remoteName = Split-Path -Leaf $LocalFile
    $remoteHash = (ssh -i "$SshKey" -o ConnectTimeout=30 $ServerName "sha256sum ~/$remoteName | cut -d' ' -f1").Trim()
    if ($remoteHash -eq $localHash) {
        Write-Host "    ✅ SHA-256 verificado" -ForegroundColor Green
        return $true
    } else {
        Write-Host "    ❌ SHA-256 no coincide (local=$localHash remoto=$remoteHash)" -ForegroundColor Red
        return $false
    }
}

foreach ($server in $Servers) {
    $name = $server.Name
    Write-Host "  → $name ..." -ForegroundColor Yellow

    if (-not (Send-File -ServerName $name -LocalFile $TarFile)) {
        Write-Host "    ❌ Subida falló en $name. Abortando deploy." -ForegroundColor Red
        exit 1
    }
}

# ── 4. Extraer, compilar e instalar systemd en cada servidor ──
Write-Host "`n[4/4] Compilando e instalando systemd en servidores..." -ForegroundColor Yellow

foreach ($server in $Servers) {
    $name = $server.Name
    $port = $server.Port
    $isSeed = $port -ne "0"

    Write-Host "  → $name (listen-port: $port) ..." -ForegroundColor Yellow

    # Comando SSH: extraer tar, compilar, instalar service file
    $sshCmd = @"
cd /home/user && tar xf ledgerita_core.tar && rm -f /home/user/ledgerita_core/.cargo/config.toml && cd /home/user/ledgerita_core && source ~/.cargo/env && cargo build --release && if [ ! -f /etc/systemd/system/ledgerita-core.service ]; then sudo cp scripts/ledgerita-core.service /etc/systemd/system/; fi && sudo mkdir -p /etc/ledgerita && if [ ! -f /etc/ledgerita/env ]; then echo 'LEDGERITA_PASSWORD=REEMPLAZAR_CON_CLAVE' | sudo tee /etc/ledgerita/env && sudo chmod 600 /etc/ledgerita/env; fi && sudo systemctl daemon-reload && sudo systemctl enable ledgerita-core
"@

    try {
        ssh -i "$SshKey" -o ConnectTimeout=30 $name $sshCmd
        Write-Host "    ✅ Build e instalación completados" -ForegroundColor Green
    } catch {
        Write-Host "    ⚠️  Error en $name : $_" -ForegroundColor Red
        continue
    }
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Deploy completado." -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "⚠️  Próximos pasos MANUALES:" -ForegroundColor Yellow
Write-Host "  1. Editar contraseña en cada servidor:"
Write-Host "     ssh -i `"$SshKey`" kettle03 'sudo nano /etc/ledgerita/env'"
Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo nano /etc/ledgerita/env'"
Write-Host ""
Write-Host "  2. (Opcional) Editar service file de kettle02 para agregar --connect:"
Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo nano /etc/systemd/system/ledgerita-core.service'"
Write-Host "     Agregar a ExecStart: --connect /ip4/192.168.68.61/tcp/36565/p2d/12D3KooW..."
Write-Host "     Luego: sudo systemctl daemon-reload && sudo systemctl restart ledgerita-core"
Write-Host ""
Write-Host "  3. Iniciar servicios:"
Write-Host "     ssh -i `"$SshKey`" kettle03 'sudo systemctl start ledgerita-core'"
Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo systemctl start ledgerita-core'"
Write-Host ""
Write-Host "  4. Ver logs:"
Write-Host "     ssh -i `"$SshKey`" kettle03 'journalctl -u ledgerita-core -f'"
