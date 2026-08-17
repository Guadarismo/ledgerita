#!/bin/bash
# Ledgerita — Healthcheck de nodos (is-active + sync + alerta)
# Comprueba que los servicios systemd estén activos y que cada nodo tenga
# actividad P2P reciente (pings/sync) en los logs.
#
# - No requiere sudo para `systemctl is-active`.
# - Requiere sudo NOPASSWD para el wrapper `ledgerita-journal` (ver
#   ledgerita-healthcheck.sudoers). Si el journal no es accesible, degrada a
#   solo-check de is-active y lo anota en el log.
# - Alerta por transición de estado (down/up) para evitar spam. Opcionalmente
#   envía webhook (Telegram/Discord/Slack) si existe el archivo:
#     /home/user/ledgerita-webhook.url   (contenido: URL del webhook)
#
# Uso (cron):
#   */15 * * * * /home/user/ledgerita-healthcheck.sh
set -uo pipefail

LOG=/home/user/backups/ledgerita/healthcheck.log
STATE=/home/user/backups/ledgerita/healthcheck.state
WEBHOOK_URL_FILE=/home/user/ledgerita-webhook.url
SYNC_WINDOW_MIN=15
SERVICES=(ledgerita-core ledgerita-core-testnet)
PATTERN="Ping exitoso|Conexión establecida|Estado sincronizado|Estado ya sincronizado|Nodo escuchando"

mkdir -p "$(dirname "$LOG")"
: > "$STATE.tmp"

now() { date '+%Y-%m-%d %H:%M:%S'; }
log() { echo "$(now) $*" >> "$LOG"; }

webhook() {
    local msg="$1"
    if [ -f "$WEBHOOK_URL_FILE" ]; then
        local url
        url=$(head -n1 "$WEBHOOK_URL_FILE" 2>/dev/null | tr -d '[:space:]')
        if [ -n "$url" ]; then
            curl -fsS -m 10 -H 'Content-Type: application/json' \
                -d "{\"text\": \"$(printf '%s' "$msg" | sed 's/"/\\"/g')\"}" \
                "$url" >/dev/null 2>&1 \
                || log "WARN webhook falló (url: ${url:0:30}...)"
        fi
    fi
}

# Transición de estado: solo alerta cuando cambia respecto al último run.
# $1 = clave (servicio), $2 = estado actual (ok|down), $3 = mensaje descriptivo
report() {
    local key="$1" status="$2" msg="$3"
    echo "$key=$status" >> "$STATE.tmp"
    local prev=""
    [ -f "$STATE" ] && prev=$(grep "^$key=" "$STATE" | cut -d= -f2)
    if [ "$prev" != "$status" ]; then
        if [ "$status" = "down" ]; then
            log "ALERTA [$key] DOWN → $msg"
            webhook "🔴 [$key] DOWN → $msg"
        else
            log "RECUPERADO [$key] $msg"
            webhook "🟢 [$key] RECUPERADO → $msg"
        fi
    elif [ "$status" = "down" ]; then
        log "PERSISTE [$key] $msg"
    fi
}

for svc in "${SERVICES[@]}"; do
    active=$(systemctl is-active "$svc" 2>/dev/null || echo "unknown")

    if [ "$active" != "active" ]; then
        report "$svc" "down" "$svc NO está activo (estado: $active)"
        continue
    fi

    # Sync: actividad P2P reciente en el journal (vía wrapper sudo NOPASSWD).
    # Se captura en variable primero: con `set -o pipefail`, grep -q corta el
    # pipe con SIGPIPE y `sudo` reporta error → falso "sin sync".
    out=""
    out=$(sudo -n /usr/local/bin/ledgerita-journal "$svc" 2>/dev/null)

    if [ -n "$out" ] && printf '%s' "$out" | grep -Eq "$PATTERN"; then
        report "$svc" "ok" "$svc activo y con actividad P2P (últimos ${SYNC_WINDOW_MIN} min)"
    elif [ -n "$out" ]; then
        report "$svc" "down" "$svc activo pero SIN actividad P2P en los últimos ${SYNC_WINDOW_MIN} min (sync comprometida)"
    else
        log "WARN [$svc] journal no accesible sin sudo (instalar ledgerita-healthcheck.sudoers). Solo chequeo is-active."
        report "$svc" "ok" "$svc activo (journal no accesible, sync no verificada)"
    fi
done

mv "$STATE.tmp" "$STATE"
exit 0
