# 📜 Índice de sesiones — Ledgerita

Historial completo de sesiones. Cada entrada: fecha, archivo, resumen en una línea.
El handoff **vigente** está en `../HANDOFF.md` (raíz); las versiones anteriores
se archivan aquí con `nueva-sesion.ps1 -Archivar`.

| Fecha | Archivo | Resumen |
|---|---|---|
| 2026-06-15 | `sesion-2026-06-15.md` | Primera sesión: seteo del entorno, arranque del repo y reporte de apertura del proyecto |
| 2026-06-16 | `sesion-2026-06-16.md` | Primer respaldo: Ledgerita Core Fases 1-3 (módulo contable, consenso de estado, sync P2P) |
| 2026-06-16 | `REPORTE_SESION_2026-06-16.md` | Persistencia del estado + base del CLI |
| 2026-06-17 | `REPORTE_SESION_2026-06-17.md` | Wallet CLI (`--send`) + preparación de la sesión 2 |
| 2026-06-17 | `REPORTE_SESION_2026-06-17-v2.md` | Detalle técnico de la segunda mitad de la sesión |
| 2026-06-17 | `REPORTE_SESION_2026-06-17-v3.md` | Cierre del día: estado consolidado |
| 2026-06-18 | `REPORTE_SESION_2026-06-18.md` | Iteración diaria: fixes y tests |
| 2026-06-18 | `REPORTE_SESION_2026-06-18-v2.md` | Continuación |
| 2026-06-19 | `REPORTE_SESION_2026-06-19.md` | Wallet persistente + conexión activa P2P + dialing + CLI flags (commit `12eb2f1`) |
| 2026-06-19 | `REPORTE_SESION_2026-06-19-v2.md` | Fix stale keys Sled + stress test P2P |
| 2026-06-19 | `REPORTE_SESION_2026-06-19-v3.md` | Test fork/resolución P2P + reporte completo |
| 2026-06-19 | `REPORTE_FINAL_CONTEXTO.md` | 📌 Documento maestro de arquitectura e historia (7 módulos, dependencias, produccion) |
| 2026-06-15 | `opinion de deepseek del proyecto 15-06-2026.txt` | Revisión externa del proyecto (DeepSeek) |
| 2026-07-03 | `REPORTE_SESION_2026-07-03.md` | Test reconexión P2P + test manual producción |
| 2026-07-03 | `REPORTE_SESION_2026-07-03-v2.md` | Fix mint tx_count + retry dial P2P |
| 2026-07-03 | `REPORTE_FINAL_SESION_2026-07-03.md` | Reporte final del día: deploy fixes (IPs, tar, SFTP, BOM) + helpers P2P IPv6/labels |
| 2026-08-02 | `REPORTE_SESION_2026-08-02.md` | Unificar SCP/SFTP en deploy.ps1 (Send-File + SHA-256) + deploy real producción |
| 2026-08-02 | `REPORTE_SESION_2026-08-02_1137.md` | Baja prioridad: ResponseSent a trace + docstrings + script vendor-local-deps.ps1 |
| 2026-08-02 | `REPORTE_HANDOFF_2026-08-02.md` | 📌 Handoff de esa sesión: TestNet formal (chain id + genesis faucet + deploy parametrizado) |
| 2026-08-02 | `REPORTE_SESION_2026-08-02_1137.md` | Credenciales/traps completos + auditoría crypto OK + backups con cron |
| 2026-08-17 | `REPORTE_SESION_2026-08-17.md` | Healthcheck + fix bug reconexión P2P + GUI wallet (egui) + monitor TUI (ratatui) |
| 2026-08-17 | `REPORTE_HANDOFF_2026-08-17_1305.md` | 📌 Handoff 13:05 autocontenido (último antes de la nueva convención HANDOFF.md) |
| 2026-11-05 | `REPORTE_SESION_2026-11-05_1714.md` | Revisión integral del proyecto + mejora del sistema de handoff (HANDOFF.md fijo, historial en handoffs/, plantilla+generador, secretos a SECRETS_LOCAL.md) |
| 2026-11-11/12 | `REPORTE_SESION_2026-11-05_1714.md` (cont.) | 🔥 Sesión gigante (Ckpts 27–35): opiniones cripto (27/28) → **CEREMONIA MainNet** con primer Cierre real + goteo 500 LIT (29) → deuda 9a funnel hash + snapshot v4 (30) → deploy v4 (31) → **autoupdate completo F1+F2+F3** con swaps reales en producción (32/33/34, U1-frase + U4-canario) → diseños 9b/9c (35) |

---

## Notas de uso

- **Para retomar el proyecto:** leer `../HANDOFF.md` (raíz) de arriba a abajo.
- **Para detalles de una sesión:** abrir el archivo de esa fecha; si el resumen no basta, el commit de git (`git log --oneline --all`) da el detalle.
- **Al cerrar una sesión nueva:** `./nueva-sesion.ps1` genera el reporte desde la plantilla y `-Archivar` mueve el HANDOFF.md viejo aquí.
