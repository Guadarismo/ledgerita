# ✅ Reporte de Sesión — Ledgerita (2026-11-05)
**Fecha y hora:** 17:14 → ampliación ~17:45 (hora local)
**Branch:** main
**Último commit al inicio:** a446074 Reporte de handoff 2026-08-17 13:05: GUI wallet + monitor TUI + healthcheck + fix P2P, autocontenido para nueva sesion
**Estado de git al cierre:** pendiente de commit (21 renames + 5 archivos nuevos + 1 edit)
**Tipo de sesión:** `/review` — revisión integral del proyecto (sin tocar código de producción) + mejora del sistema de handoff + auditoría informal que descubrió deuda técnica nueva (sección ⚠️ de este reporte)

> ⚠️ SOBRE ESTE REPORTE: la **deuda técnica descubierta leyendo el código** (sección
> "Deuda técnica — hallazgos NUEVOS") NO está documentada en ningún otro reporte
> del proyecto. Un análisis de `inicio.md` (la visión) contra el código real.
> La próxima sesión puede arrancar con este reporte + `HANDOFF.md` solamente.

---

## 🎯 Resumen de lo hecho

1. **Revisión integral del proyecto**: leídos el núcleo completo (`ledgerita_core/src/*`, bins), los 3 crates (wallet egui, TUI ratatui), `deploy.ps1`, los scripts de producción (healthcheck, journal wrapper, sudoers, backup) y todo el historial de reportes.
2. **Mejora 1 — `HANDOFF.md` de nombre fijo**: creado en la raíz como única puerta de entrada para una sesión nueva (siempre contiene el handoff vigente).
3. **Mejora 2 — historial ordenado**: 21 reportes movidos con `git mv` (renames preservados en git) a `handoffs/`, con `handoffs/INDEX.md` (una línea por sesión). La raíz quedó con lo operativo: `HANDOFF.md`, `inicio.md`, `PLANTILLA_SESION.md`, `nueva-sesion.ps1`, `SECRETS_LOCAL.md`, `deploy.ps1` y el código.
4. **Mejora 3 — plantilla + generador**: `PLANTILLA_SESION.md` (secciones estándar) y `nueva-sesion.ps1` (crea el reporte pre-rellenado con branch/commit/estado de git reales; `-Archivar` mueve el HANDOFF.md viejo a `handoffs/` y crea el esqueleto del nuevo). Probado en vivo: generó este mismo archivo.
5. **Mejora 4 — credenciales fuera del repo**: creado `SECRETS_LOCAL.md` (NO versionado, añadido a `.gitignore`) como única fuente local de passwords (sudo, wallet, env de nodos, SSH). Scrub en bloque de los 15 reportes históricos: `obegt…` → `<VER_SECRETS_LOCAL.md>` y `clavefacil` → `<SUDO_PASSWORD>`.
6. **Ignore global**: `%USERPROFILE%\.gitignore_global` con `SECRETS_LOCAL.md` + `wallet_recovery.env`, registrado en `core.excludesFile` → aplica a todos los repos del PC (blindaje contra `git add -f`).

---

## 🔧 Cambios técnicos (detalle)

- **Creados:** `HANDOFF.md`, `PLANTILLA_SESION.md`, `nueva-sesion.ps1`, `SECRETS_LOCAL.md`, `handoffs/INDEX.md` y este reporte.
- **Movidos:** todos los `REPORTE_*.md`, `sesion-*.md` y la `opinion de deepseek…` a `handoffs/` vía `git mv`.
- **Editados:** `.gitignore` (+`SECRETS_LOCAL.md`); los reportes históricos de `handoffs/` (solo sustitución de secretos, sin cambio de contenido); `%USERPROFILE%\.gitignore_global` (fuera del repo).
- **Sin tocar:** todo el código (`ledgerita_core`, `ledgerita_wallet`, `ledgerita_monitor_tui`), `deploy.ps1` y scripts de producción.

## 🖥️ Producción (si aplica)

Ningún cambio de producción; la sesión no tocó servidores (kettle03/kettle02 quedan como el handoff 2026-08-17).

---

## 🧪 Tests

```
Sin cambios de código → estado previo vigente: 79 passed
(70 core release + 4 wallet + 5 monitor TUI)
```

## 🗂️ Git

Cambios pendientes de commit. Sugerencia de mensaje:
`Mejor sistema de handoff: HANDOFF.md fijo + historial en handoffs/ con INDEX + plantilla/generador de reportes + credenciales a SECRETS_LOCAL.md (repo limpio de secretos)`

---

## 🚨 (⚠️ NUEVO — no está en ningún reporte anterior) Deuda técnica del núcleo

> Detectada el 2026-11-05 contrastando `inicio.md` (la visión del protocolo) contra el código real.

### A. El reparto de fees a "validadores" NO existe (placeholder) — el gap más serio vs la visión
- **Dónde:** `ledgerita_core/src/state.rs` líneas 144-158 (`finalize_checkpoint`) y sus dos llamadas en `network.rs` (líneas ~279 y ~428).
- **Realidad:** SIEMPRE se llama con `std::slice::from_ref(&wallet_address)` — el `fee_pool` se reparte 100% al nodo local. Nunca se reparte entre un pool de validadores reales.
- **Riesgo si se arregla mal:** tocar `finalize_checkpoint` cambia saldos → cambia el `generate_state_root()` → los peers se verían **Divergent** hasta que todos migren. Es un cambio de consenso: requiere versión o desfase coordinado.

### B. No hay "baneo/expulsión" de nodos engañosos (prometido en inicio.md, faltante)
- **Dónde:** `network.rs` — la visión dice "Cualquier nodo que emita un resultado inválido es inmediatamente ignorado y expulsado (baneado)". El código solo hace `warn!` + `send_response(b"INVALID…")`. No hay ban list, ni reputación, ni kick.

### C. "Prueba de Tiempo" (reputación / anti-Sybil): cero implementación
- La visión promete "los nodos ganan peso por estabilidad y antigüedad". Hoy: nada. B y C comparten el mismo diseño de fondo (reputación de nodos), conviene abordarlos juntos.

### D. Semántica del `cap` del faucet es rara
- **Dónde:** `main.rs` línea 260: `s.apply_faucet(&storage, network, &addr, *amount, faucet_amount)` — el 4º argumento es **amount**, el 5º es **cap**, y a ambos se les pasa la misma variable `faucet_amount`.
- **Efecto:** el tope por dirección = el monto dado en cada llamada → faucet de un solo uso por dirección, y el cap no es un valor global configurable. Sugerencia: flag `--faucet-cap <n>` separado del monto por entrega.
- La lógica base en `faucet.rs` es correcta (`given + amount > cap` → rechaza); el problema es sólo quién le pasa el cap.

### E. Tesoro TestNet nace de seed cero — JAMÁS reutilizar el patrón en MainNet
- **Dónde:** `genesis.rs` líneas 21-23: `SigningKey::from_bytes(&[0u8; 32])`.
- **Por qué importa:** "sin clave privada conocida" es una ilusión: cualquiera que lea el código puede derivar la privada (la seed es pública). Aceptable en TestNet; prohibido futuras semillas de tesoro para MainNet por este estilo determinista a no ser que la seed secreta quede fuera del código (recovery/hardware), NUNCA hardcoded en `genesis.rs`.

### F. Toolchain Rust vetado para el upgrade de eframe (solo nota)
- Actual: `eframe 0.35.0` (la 0.36 exige Rust 1.95; el entorno tiene 1.94). Si alguna versión futura rompe el build, revisar los 3 puntos de la API: `App::ui`, `CentralPanel::show(ui, ...)`, `ctx.copy_text`.

---

## 🚦 Plan sugerido (pendiente de validar con el usuario en la próxima sesión)

1. **Ya (rápido):** commit de hoy + fix typo `deploy.ps1` L203 (`p2d` → `p2p`).
2. **Fase 2 punto 1 — broadcast P2P desde la wallet** (siguiente sesión propuesta). Ver justificación en `HANDOFF.md` → "Pendientes".
3. **Deuda técnica (sección anterior):** sesiones dedicadas DESPUÉS de Fase 2 punto 1, porque A/B/C son cambios de semántica de consenso/red (romper el trabajo de la wallet en plena migración sería caro).
   - Orden interno sugerido: **D** (faucet cap, trivial) → **A** (fee pool / validators, requiere diseño propio) → **B + C** (baneo + Prueba de Tiempo, juntos por diseño compartido de reputación) → **E** (solo documentar/advertir ahora) → **F** (opcional, cuando toque upgrade de deps).
4. **Resto de Fase 2** (sync en vivo en la wallet, historial de TX, botón faucet) → Fase 3 (pulido GUI).

## 🚀 Pendientes para la próxima (catálogo completo)

### Inmediato (esta semana)
1. **Commit** de los cambios de hoy.
2. **Fix typo `deploy.ps1`** L203: `p2d` → `p2p`.
3. (Opcional) Purgar secretos del historial git (solo si el repo va a ser público).

### Fase 2 — wallet útil (próxima natural)
1. Broadcast P2P de TX firmadas (desde GUI).
2. Sync en vivo desde nodo remoto (patrón del TUI `MSG_SYNC`).
3. Historial de TX.
4. Botón faucet TestNet.

### Deuda técnica → ver sección 🚨 de arriba (A-F con orden interno sugerido).

### Fase 3 — pulido GUI
QR dirección, dark mode, selector red en dashboard, binario portable, WASM (opcional).

### Infra/opcional
- Webhook real (`/home/user/ledgerita-webhook.url`).
- Observar estabilidad reconexión P2P a largo plazo (journal, semanas).
- `--data-dir` en el monitor TUI (modo local).
- Faucet como servicio HTTP o mensaje P2P (`faucet.rs` ya está aislado para eso).
- Limpieza de raíz: `ledgerita_core.tar`, `add_error.log` desechables.
- (Solo si repo va a ser público) `git filter-repo`/BFG para secretos en commits viejos.

## ⚠️ Trampas descubiertas en esta sesión

1. **`git mv` de varios archivos a un destino aún inexistente falla** ("destination is not a directory"): crear la carpeta antes (`New-Item -ItemType Directory`) y después mover todo.
2. El scrub por PowerShell debe escribir con `UTF8Encoding($false)` (sin BOM) y `WriteAllText` para no alterar la codificación de los `.md` históricos.
3. `git check-ignore -v` siempre reporta la regla local primero; para verificar el ignore global inspeccionar `%USERPROFILE%\.gitignore_global` + `git config --global --get core.excludesFile`.
