#!/bin/bash
# Ledgerita — Wrapper read-only para el healthcheck.
# Permite a `user` leer los journals de los servicios de Ledgerita sin password,
# sin dar acceso a journalctl genérico (mínimo privilegio).
#
# Uso:  ledgerita-journal <unit>
# Solo acepta las unidades de Ledgerita y fija la ventana a los últimos 15 min.
#
# Instalación (root):
#   sudo cp ledgerita-journal.sh /usr/local/bin/ledgerita-journal
#   sudo chmod 755 /usr/local/bin/ledgerita-journal
set -u

case "${1:-}" in
    ledgerita-core|ledgerita-core-testnet) ;;
    *)
        echo "unidad no permitida: ${1:-}" >&2
        exit 1
        ;;
esac

exec /usr/bin/journalctl -u "$1" --since '15 min ago' --no-pager
