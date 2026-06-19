#!/bin/bash
set -e

echo "========================================="
echo " Ledgerita — Systemd Setup"
echo "========================================="
echo ""

# ── 1. Crear environment file con la contraseña ──
if [ ! -f /etc/ledgerita/env ]; then
    echo "Creando /etc/ledgerita/env ..."
    sudo mkdir -p /etc/ledgerita
    echo "# Ledgerita wallet password" | sudo tee /etc/ledgerita/env > /dev/null
    echo "LEDGERITA_PASSWORD=REEMPLAZAR_CON_CLAVE_SEGURA" | sudo tee -a /etc/ledgerita/env > /dev/null
    sudo chmod 600 /etc/ledgerita/env
    echo "⚠️  EDITA /etc/ledgerita/env con la contraseña REAL antes de iniciar el servicio."
    echo "   Ejemplo: sudo nano /etc/ledgerita/env"
    echo ""
else
    echo "✅ /etc/ledgerita/env ya existe."
fi

# ── 2. Instalar service file ──
SERVICE_SRC="$(dirname "$0")/ledgerita-core.service"
if [ -f "$SERVICE_SRC" ]; then
    sudo cp "$SERVICE_SRC" /etc/systemd/system/ledgerita-core.service
    echo "✅ Service file instalado en /etc/systemd/system/ledgerita-core.service"
else
    echo "⚠️  ledgerita-core.service no encontrado en el directorio del script."
    echo "   Copia manual: sudo cp ledgerita-core.service /etc/systemd/system/"
fi

# ── 3. Recargar systemd, habilitar servicio ──
sudo systemctl daemon-reload
sudo systemctl enable ledgerita-core
echo ""
echo "✅ Servicio habilitado (arranca automáticamente al iniciar el sistema)."
echo ""
echo "========================================="
echo " Comandos útiles:"
echo "   Iniciar ahora:  sudo systemctl start ledgerita-core"
echo "   Ver estado:     systemctl status ledgerita-core"
echo "   Logs en vivo:   journalctl -u ledgerita-core -f"
echo "   Detener:        sudo systemctl stop ledgerita-core"
echo "   Reiniciar:      sudo systemctl restart ledgerita-core"
echo "========================================="
