# ─────────────────────────────────────────────────────
# escanear_lan.ps1 — Escáner de LAN (toolbox Ledgerita)
#
# Encuentra hosts vivos en una subred con TCP-connect scan
# asincrónico (254 IPs × N puertos en ~1 segundo, cero
# dependencias externas) y muestra la MAC de cada vivo.
#
# Uso típico:
#   .\escanear_lan.ps1                        # subred default, puertos 22/80/9100
#   .\escanear_lan.ps1 -Subnet 10.0.0         # otra subred
#   .\escanear_lan.ps1 -Ports 22              # solo SSH (kettles, artefactos de lab)
#   .\escanear_lan.ps1 -Ports 9100            # solo impresoras 3D (raw 9100)
#
# Nota: TCP-connect NO depende de ICMP — un cortafuegos que
# bloquea el ping no la frena. Solo lectura: a lo sumo envía los
# SYN de connect de siempre.
# ─────────────────────────────────────────────────────

param(
    [string]$Subnet = "192.168.68",
    [int[]]$Ports = @(22, 80, 9100),
    [int]$TimeoutMs = 400
)

$ErrorActionPreference = "SilentlyContinue"

# 1. Refrescar la tabla ARP con un ping al broadcast (barato).
$null = ping -n 1 -w 250 "$Subnet.255" 2>$null

# 2. Lanzar TODOS los connects asincrónicos de una sola vez.
$all = New-Object System.Collections.Generic.List[object]
foreach ($n in 1..254) {
    $ip = "$Subnet.$n"
    foreach ($p in $Ports) {
        $c = New-Object System.Net.Sockets.TcpClient
        $task = $c.ConnectAsync($ip, $p)
        $all.Add([pscustomobject]@{ IP = $ip; Port = $p; Cliente = $c; Tarea = $task })
    }
}

# 3. Esperar la ventana de timeout (los caídos fallan rápido).
Start-Sleep -Milliseconds ($TimeoutMs + 150)

# 4. Cosechar: conectada sin error ⇒ puerto abierto.
$abiertos = @{}
foreach ($r in $all) {
    $ok = $false
    try { $ok = ($r.Tarea.IsCompleted -and (-not $r.Tarea.IsFaulted) -and $r.Cliente.Connected) } catch { $ok = $false }
    if ($ok) {
        if (-not $abiertos.ContainsKey($r.IP)) { $abiertos[$r.IP] = New-Object System.Collections.Generic.List[int] }
        $abiertos[$r.IP].Add($r.Port)
    }
    $r.Cliente.Dispose()
}

# 5. MACs desde la tabla ARP.
$macs = @{}
Get-NetNeighbor -AddressFamily IPv4 | ForEach-Object {
    $v = $_.IPAddress
    if ($v -like "$Subnet.*" -and ($_.State -eq 'Reachable' -or $_.State -eq 'Stale')) {
        $macs[$v] = $_.LinkLayerAddress
    }
}

# 6. Reporte.
function ip-sor([string]$ip) { [System.Version]$ip }   # orden numérico real de IPs

Write-Host ""
Write-Host ("TCP scan {0}.1–{0}.254  ·  puertos: {1}" -f $Subnet, ($Ports -join ', ')) -ForegroundColor Cyan
Write-Host ("─" * 62)

if ($abiertos.Count -eq 0) {
    Write-Host "  (nadie contestó — revisá la subred o los puertos)" -ForegroundColor Yellow
}

foreach ($ip in ($abiertos.Keys | Sort-Object ip-sor)) {
    $lista = ($abiertos[$ip] | Sort-Object) -join ', '
    $mac = if ($macs.ContainsKey($ip)) { $macs[$ip] } else { "- (sin ARP)" }
    ("  {0,-16}  MAC {1,-18}  puertos: {2}" -f $ip, $mac, $lista) | Write-Host -ForegroundColor Green
}

Write-Host ("─" * 62)
Write-Host ("Hosts con puertos abiertos: {0} de 254" -f $abiertos.Count) -ForegroundColor Cyan
Write-Host "  · con 22 abierto  = Linux/SSH   → probable kettle/artefacto del lab" -ForegroundColor Gray
Write-Host "  · con 9100 o 80   = impresora 3D o webUI de router" -ForegroundColor Gray
