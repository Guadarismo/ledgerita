# ─────────────────────────────────────────────────────
# Ledgerita Deploy Script
# Empaqueta, sube, compila e instala systemd en servidores
# ─────────────────────────────────────────────────────

param(
    [ValidateSet("mainnet", "testnet")]
    [string]$Network = "mainnet",
    [string]$SeedPeerId = "",
    [switch]$ForceUpdateUnit
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$TarFile = Join-Path $ProjectRoot "ledgerita_core.tar"
$SshKey = "$env:USERPROFILE\.ssh\id_ed25519_ledgerita"

# Servidores de producción
if ($Network -eq "testnet") {
    # TestNet comparte hosts con MainNet pero con puerto, binario, data-dir y servicio propios
    $Servers = @(
        @{ Name = "kettle03"; Ip = "192.168.68.60"; Port = "46565"; Role = "seed" }
        @{ Name = "kettle02"; Ip = "192.168.68.64"; Port = "0"; Role = "client" }
    )
} else {
    $Servers = @(
        @{ Name = "kettle03"; Ip = "192.168.68.60"; Port = "36565"; Role = "seed" }
        @{ Name = "kettle02"; Ip = "192.168.68.64"; Port = "0"; Role = "client" }
    )
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Ledgerita Deploy" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ── 1. Compilar y testear (local) ──
Write-Host "`n[1/4] Compilando y testeando localmente..." -ForegroundColor Yellow
Set-Location (Join-Path $ProjectRoot "ledgerita_core")
cargo test --offline --release --locked
if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Tests fallaron. Abortando deploy." -ForegroundColor Red
    exit 1
}
cargo build --offline --release --locked
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
    $localHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $LocalFile).Hash.ToLowerInvariant()
    $remoteName = Split-Path -Leaf $LocalFile
    $remoteHash = (ssh -i "$SshKey" -o ConnectTimeout=30 $ServerName "sha256sum ~/$remoteName | cut -d' ' -f1").Trim().ToLowerInvariant()
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
    $role = $server.Role
    $isSeed = $port -ne "0"

    $serviceName = if ($Network -eq "testnet") { "ledgerita-core-testnet" } else { "ledgerita-core" }
    $unitPath = "/etc/systemd/system/${serviceName}.service"
    $forceUnitFlag = if ($ForceUpdateUnit) { "1" } else { "0" }

    if ($Network -eq "testnet") {
        $extraFlags = "--network testnet --data-dir /home/user/.ledgerita-testnet"
        if ($isSeed) {
            $extraFlags += " --listen-port $port"
        } elseif ($SeedPeerId) {
            $extraFlags += " --connect /ip4/192.168.68.60/tcp/46565/p2p/$SeedPeerId"
        }
        Write-Host "  → $name (TestNet → ledgerita_core_testnet, listen-port: $port) ..." -ForegroundColor Yellow

        $sshCmd = @"
set -e
cd /home/user
tar xf ledgerita_core.tar
rm -f /home/user/ledgerita_core/.cargo/config.toml
cd /home/user/ledgerita_core
source ~/.cargo/env
cargo build --release --locked --bin ledgerita_core --target-dir /home/user/ledgerita_core/target_testnet
mkdir -p /home/user/ledgerita_core/target/release
cp /home/user/ledgerita_core/target_testnet/release/ledgerita_core /home/user/ledgerita_core/target/release/ledgerita_core_testnet.tmp
chmod 755 /home/user/ledgerita_core/target/release/ledgerita_core_testnet.tmp
mv -f /home/user/ledgerita_core/target/release/ledgerita_core_testnet.tmp /home/user/ledgerita_core/target/release/ledgerita_core_testnet
sudo mkdir -p /etc/ledgerita
if [ ! -f /etc/ledgerita/env ]; then
    printf 'LEDGERITA_PASSWORD=REEMPLAZAR_CON_CLAVE\nRUST_LOG=ledgerita_core=info,warn\n' | sudo tee /etc/ledgerita/env > /dev/null
    sudo chmod 600 /etc/ledgerita/env
fi
if [ ! -f "$unitPath" ] || [ "$forceUnitFlag" = "1" ]; then
cat > /home/user/${serviceName}.service <<'UNIT'
[Unit]
Description=Ledgerita Protocol Core Node (TestNet)
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=user
Group=user
WorkingDirectory=/home/user/ledgerita_core
ExecStart=/home/user/ledgerita_core/target/release/ledgerita_core_testnet $extraFlags
EnvironmentFile=/etc/ledgerita/env
Restart=always
RestartSec=10
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
UNIT
sudo cp /home/user/${serviceName}.service "$unitPath"
else
# Preservar flags existentes de validador/faucet (--validators-file, --validator-id, --validator-key-file); solo alinear nombre de binario TestNet si apuntaba al binario MainNet.
sudo sed -i 's|/home/user/ledgerita_core/target/release/ledgerita_core |/home/user/ledgerita_core/target/release/ledgerita_core_testnet |g' "$unitPath"
fi
sudo systemctl daemon-reload
sudo systemctl enable ${serviceName}
"@
    } else {
        Write-Host "  → $name (MainNet → ledgerita_core, listen-port: $port) ..." -ForegroundColor Yellow

        # Comando SSH: compilar en target_mainnet e instalar atómicamente; preservar service existente y sus flags de validador.
        $sshCmd = @"
set -e
cd /home/user
tar xf ledgerita_core.tar
rm -f /home/user/ledgerita_core/.cargo/config.toml
cd /home/user/ledgerita_core
source ~/.cargo/env
cargo build --release --locked --bin ledgerita_core --target-dir /home/user/ledgerita_core/target_mainnet
mkdir -p /home/user/ledgerita_core/target/release
cp /home/user/ledgerita_core/target_mainnet/release/ledgerita_core /home/user/ledgerita_core/target/release/ledgerita_core.tmp
chmod 755 /home/user/ledgerita_core/target/release/ledgerita_core.tmp
mv -f /home/user/ledgerita_core/target/release/ledgerita_core.tmp /home/user/ledgerita_core/target/release/ledgerita_core
if [ ! -f "$unitPath" ] || [ "$forceUnitFlag" = "1" ]; then
    sudo cp scripts/ledgerita-core.service "$unitPath"
fi
sudo mkdir -p /etc/ledgerita
if [ ! -f /etc/ledgerita/env ]; then
    printf 'LEDGERITA_PASSWORD=REEMPLAZAR_CON_CLAVE\nRUST_LOG=ledgerita_core=info,warn\n' | sudo tee /etc/ledgerita/env > /dev/null
    sudo chmod 600 /etc/ledgerita/env
fi
sudo systemctl daemon-reload
sudo systemctl enable ledgerita-core
"@
    }

    $tmpDeploySh = Join-Path $env:TEMP ("ledgerita_deploy_{0}_{1}.sh" -f $Network, $name)
    try {
        [System.IO.File]::WriteAllText(
            $tmpDeploySh,
            ($sshCmd -replace "`r`n", "`n") + "`n",
            (New-Object System.Text.UTF8Encoding($false))
        )
        scp -i "$SshKey" -o ConnectTimeout=30 $tmpDeploySh "${name}:~/ledgerita_deploy.sh"
        if ($LASTEXITCODE -ne 0) {
            throw "SCP de script remoto falló (código $LASTEXITCODE)"
        }
        ssh -t -i "$SshKey" -o ConnectTimeout=30 $name "bash ~/ledgerita_deploy.sh && rm -f ~/ledgerita_deploy.sh"
        if ($LASTEXITCODE -ne 0) {
            throw "SSH de instalación falló (código $LASTEXITCODE)"
        }
        Write-Host "    ✅ Build e instalación completados" -ForegroundColor Green
    } catch {
        Write-Host "    ⚠️  Error en $name : $_" -ForegroundColor Red
        continue
    } finally {
        Remove-Item -LiteralPath $tmpDeploySh -ErrorAction SilentlyContinue
    }
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Deploy completado ($Network)." -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "⚠️  Próximos pasos MANUALES:" -ForegroundColor Yellow
Write-Host "  1. Editar contraseña en cada servidor:"
Write-Host "     ssh -i `"$SshKey`" kettle03 'sudo nano /etc/ledgerita/env'"
Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo nano /etc/ledgerita/env'"
Write-Host ""

if ($Network -eq "testnet") {
    Write-Host "  2. Iniciar el seed TestNet primero:"
    Write-Host "     ssh -i `"$SshKey`" kettle03 'sudo systemctl start ledgerita-core-testnet'"
    Write-Host "     ssh -i `"$SshKey`" kettle03 'journalctl -u ledgerita-core-testnet -n 30'"
    Write-Host "     Anotar el PeerId que aparece en el log (línea 'Identidad P2P')."
    Write-Host ""
    Write-Host "  3. Conectar el cliente kettle02 (si es instalación nueva, usar -ForceUpdateUnit):"
    Write-Host "     .\deploy.ps1 -Network testnet -SeedPeerId <PEER_ID_DEL_SEED>"
    Write-Host "     (No sobreescribe flags existentes de validador en unidades ya creadas.)"
    Write-Host ""
    Write-Host "  4. Iniciar el cliente:"
    Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo systemctl start ledgerita-core-testnet'"
    Write-Host "     ssh -i `"$SshKey`" kettle02 'journalctl -u ledgerita-core-testnet -f'"
    Write-Host ""
    Write-Host "  5. Faucet TestNet (con TX firmada bajo BFT g7):"
    Write-Host "     .\target\release\ledgerita_core --network testnet --data-dir .\testnet-data --faucet-key-file <seed> --faucet <ADDR_DESTINO> 5000 [--faucet-cap 10000]"
} else {
    Write-Host "  2. (Opcional) Editar service file de kettle02 para agregar --connect:"
    Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo nano /etc/systemd/system/ledgerita-core.service'"
    Write-Host "     Agregar a ExecStart: --connect /ip4/192.168.68.60/tcp/36565/p2p/12D3KooWMkKuW9NTuhxe8mdgPisYQgbebx3XbYKi4UG7JDCGtZE1"
    Write-Host "     Luego: sudo systemctl daemon-reload && sudo systemctl restart ledgerita-core"
    Write-Host ""
    Write-Host "  3. Iniciar servicios:"
    Write-Host "     ssh -i `"$SshKey`" kettle03 'sudo systemctl start ledgerita-core'"
    Write-Host "     ssh -i `"$SshKey`" kettle02 'sudo systemctl start ledgerita-core'"
    Write-Host ""
    Write-Host "  4. Ver logs:"
    Write-Host "     ssh -i `"$SshKey`" kettle03 'journalctl -u ledgerita-core -f'"
}
