#!/bin/bash
# Ledgerita — Backup automático (zero-downtime, sin sudo)
# Respalda state + wallet.key de mainnet y testnet a /home/user/backups/ledgerita
# Rotación: conserva los últimos 7 días.
#
# Consistencia: se copia el dir de sled en caliente. Sled es crash-safe (recupera
# su log al abrir), así que una copia a mitad de escritura se repara sola al
# restaurar; el backup simplemente puede quedar ligeramente atrás, que es lo
# correcto para DR.
#
# Instalación (cron): crontab -e →  15 4 * * * /home/user/ledgerita-backup.sh
set -euo pipefail

TS=$(date +%Y%m%d-%H%M%S)
OUT=/home/user/backups/ledgerita
mkdir -p "$OUT"

for net in ledgerita ledgerita-testnet; do
    if [ -f "/home/user/.$net/wallet.key" ]; then
        tar czf "$OUT/$net-$TS.tar.gz" -C /home/user ".$net/state" ".$net/wallet.key"
        echo "✅ $net -> $OUT/$net-$TS.tar.gz"
    fi
done

# Rotación: borrar backups con más de 7 días
find "$OUT" -name '*.tar.gz' -mtime +7 -delete

echo "🧹 Rotación completada (retención 7d)."
