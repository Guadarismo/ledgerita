//! Defensa de red node-local (unidad **B** de B+C+G, `DISENO_DEFENSA_RED.md` §2).
//!
//! [`PeerGuard`] acumula strikes por peer con una ventana rodante y aplica
//! bans EN MEMORIA DEL PROCESO. Regla constitucional: los bans/reputación
//! JAMÁS entran al `State` ni al state root (lección del hallazgo A — el
//! consenso es solo estado de cuentas; los datos node-local viven fuera).
//!
//! El tiempo entra SIEMPRE como parámetro (`now: Instant`): todas las
//! funciones son deterministas y los tests marcan el reloj a mano — nada de
//! relojes ocultos.
//!
//! Uso en ambos lados (mismo juego para todos):
//! - core: `network.rs::run_p2p_node` (demonios).
//! - wallet: `net_actor.rs::WalletActor` (wallet-node GUI).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use libp2p::PeerId;

// ── Constantes del régimen (decisión §5-③: constantes, no flags del CLI) ──

/// Ventana rodante de strikes: solo cuentan los de los últimos 10 min.
pub const STRIKE_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Strikes leves acumulados para merecer el `BAN_LIGHT_BASE`.
pub const LIGHT_STRIKE_LIMIT: u32 = 3;

/// Ventana del régimen rate-based del spam de chain_id.
pub const CHAINID_SPAM_WINDOW: Duration = Duration::from_secs(5 * 60);

/// A partir de 21 chain_id inválidos en 5 min el spam deviene grave.
/// (Los core viejos de agosto responden OK plano por versión — las leves
/// NO banean; solo la saturación de volumen, que un nodo honesto no genera.)
pub const CHAINID_SPAM_LIMIT: u32 = 20;

/// Ban por 3 strikes leves en la ventana.
pub const BAN_LIGHT_BASE: Duration = Duration::from_secs(30 * 60);
/// Ban INMEDIATO por snapshot rechazado por el gate G (decisión §5-①:
/// intentar inflar el estado es adulterio, no ruido de conexión).
pub const BAN_GATE_BASE: Duration = Duration::from_secs(60 * 60);
/// Ban por flood de requests malformados.
pub const BAN_FLOOD_BASE: Duration = Duration::from_secs(10 * 60);
/// Tope del backoff exponencial para bans repetidos.
pub const BAN_BACKOFF_MAX: Duration = Duration::from_secs(24 * 60 * 60);

/// Clase de strike (tabla §2 del diseño).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrikeKind {
    /// TX con decode/firma inválida (basura estructural o forjado — un
    /// cliente honesto jamás produce esto; los REJECT legítimos NO cuentan).
    TxInvalid,
    /// Snapshot rechazado por el gate G (§1) — grave: ban inmediato.
    SnapshotInvalido,
    /// Mensajes con chain_id/tipo desconocido — leve, con régimen rate-based
    /// por volumen (> `CHAINID_SPAM_LIMIT` en `CHAINID_SPAM_WINDOW` → grave).
    ChainIdSpam,
    /// Flood de requests malformados — grave: ban inmediato.
    RequestSpam,
}

/// Resultado de registrar un strike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BanOutcome {
    /// El strike no alcanzó un umbral: sigue hablando.
    SinBan,
    /// El peer quedó baneado por la duración indicada (incluye backoff
    /// si ya había registros de ban).
    BanCommitido(Duration),
}

// ── Unidad C: Prueba de Tiempo (métricas node-local por peer) ─────────────
//
// Evidencia acumulada de ESTABILIDAD + HONESTIDAD medida localmente. Sirve
// para: prioridad de dial, margen de strikes (peer veterano tolera 1 leve
// más), y como base de datos del selector de fees de la deuda A (3b).
// JAMÁS decide partida de fees por sí misma y JAMÁS entra al consenso:
// vive del lado node-local, en RAM (guard) + tree `peers` de Storage.

/// Margen de strikes extra para peers veteranos con uptime impecable.
/// "Veterano" aquí: ≥ 24 h de antigüedad y uptime ≥ 99% con al menos 10 pings.
pub const MARGEN_ANTIGUEDAD_MIN: Duration = Duration::from_secs(24 * 60 * 60);
pub const UPTIME_MARGEN_MIN: f64 = 0.99;
pub const PINGS_MARGEN_MIN: u64 = 10;

/// Registro persistente por peer (tree `peers` de Storage, node-local).
/// Todo u64 serializado big-endian; el orden DE SER un formato estable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerLocal {
    /// UNIX millis de la PRIMERA conexión conocida (la antigüedad es de
    /// SIEMPRE, no de la última conexión — tras re-conexiones se conserva).
    pub primera_conexion_ms: u64,
    /// Pings OK y pings fallidos acumulados (ratio en ventana de vida).
    pub pings_ok: u64,
    pub pings_fallo: u64,
    /// TX válidas aceptadas provenientes de este peer.
    pub tx_aceptadas: u64,
    /// Snapshots correctos servidos a este peer.
    pub snapshots_servidos: u64,
    /// Strikes de TODA la historia admitidos (la reputación débil queda).
    pub strikes_totales: u64,
}

impl PeerGuard {
    /// Primer (o ya) contacto establecido. La antigüedad es de SIEMPRE: la
    /// primera marca manda (re-conexiones no retroceden).
    pub fn note_conexion(&mut self, peer: &PeerId, now: Instant) {
        let s = self.scores.entry(peer.clone()).or_default();
        if s.primera_conexion.is_none() {
            s.primera_conexion = Some(now);
        }
    }

    /// Resultado del heartbeat del behaviour (§3: uptime_ratio).
    pub fn note_ping(&mut self, peer: &PeerId, ok: bool) {
        let s = self.scores.entry(peer.clone()).or_default();
        if ok {
            s.pings_ok += 1
        } else {
            s.pings_fallo += 1
        }
    }

    /// Una TX válida de este peer fue aceptada y aplicada.
    pub fn note_tx_aceptada(&mut self, peer: &PeerId) {
        self.scores.entry(peer.clone()).or_default().tx_aceptadas += 1;
    }

    /// Le servimos un snapshot válido a este peer.
    pub fn note_snapshot_servido(&mut self, peer: &PeerId) {
        self.scores
            .entry(peer.clone())
            .or_default()
            .snapshots_servidos += 1;
    }

    /// Antigüedad: tiempo desde la PRIMERA conexión conocida (conservando
    /// `edad_heredada` acumulada antes del último reinicio del SO).
    pub fn antiguedad(&self, peer: &PeerId, now: Instant) -> Option<Duration> {
        self.scores.get(peer).and_then(|s| {
            s.primera_conexion.and_then(|t0| {
                now.checked_duration_since(t0)
                    .map(|d| s.edad_heredada.saturating_add(d))
            })
        })
    }

    /// Uptime ratio: pings OK / pings totales — None sin datos aún.
    pub fn uptime_ratio(&self, peer: &PeerId) -> Option<f64> {
        let s = self.scores.get(peer)?;
        let total = s.pings_ok.saturating_add(s.pings_fallo);
        if total == 0 {
            return None;
        }
        Some(s.pings_ok as f64 / total as f64)
    }

    /// Margen de strikes extra (veterano tolera 1 leve más antes del ban).
    /// El ban por gate/flood/umbral de cadena_id es imposible de suavizar:
    /// sé viejo no te hace adulterador.
    pub fn margen_strikes(&self, peer: &PeerId, now: Instant) -> u32 {
        let vetusto = self
            .antiguedad(peer, now)
            .is_some_and(|a| a >= MARGEN_ANTIGUEDAD_MIN);
        let confiable = self
            .uptime_ratio(peer)
            .is_some_and(|r| r >= UPTIME_MARGEN_MIN);
        let ping_datos = self
            .scores
            .get(peer)
            .is_some_and(|s| s.pings_ok.saturating_add(s.pings_fallo) >= PINGS_MARGEN_MIN);
        if vetusto && confiable && ping_datos {
            1
        } else {
            0
        }
    }

    /// Orden de dial: veteranos primero (antigüedad descendente), los
    /// desconocidos al final — el intento más caro (reconexión) va al
    /// peer más estable de la historia local.
    pub fn prioridad_dial(&self, peers: &[PeerId], now: Instant) -> Vec<PeerId> {
        let mut orden = peers.to_vec();
        orden.sort_by_key(|p| std::cmp::Reverse(self.antiguedad(p, now)));
        orden
    }

    /// Snapshot persistible de TODO el guard (para el tree `peers`).
    /// `now_wall_ms` = UNIX millis de AHORA (convierte Instant → wall clock).
    pub fn export_records(&self, now: Instant, now_wall_ms: u64) -> HashMap<String, PeerLocal> {
        self.scores
            .iter()
            .filter(|(_, s)| !Self::score_sin_datos(s))
            .map(|(pid, s)| {
                let mut rec = PeerLocal {
                    primera_conexion_ms: 0,
                    pings_ok: s.pings_ok,
                    pings_fallo: s.pings_fallo,
                    tx_aceptadas: s.tx_aceptadas,
                    snapshots_servidos: s.snapshots_servidos,
                    strikes_totales: s.strikes_totales,
                };
                if let Some(edad) = self.antiguedad(pid, now) {
                    let edad_ms = edad.as_millis().min(u128::from(u64::MAX)) as u64;
                    rec.primera_conexion_ms = now_wall_ms.saturating_sub(edad_ms);
                }
                (pid.to_string(), rec)
            })
            .collect()
    }

    /// Importa los registros del árbol `peers` (arranque del nodo): la
    /// antigüedad SIEMPRE sobrevive al reinicio, incluso si el SO lleva menos
    /// tiempo encendido que la antigüedad del peer.
    pub fn import_records(
        &mut self,
        datos: HashMap<String, PeerLocal>,
        now: Instant,
        now_wall_ms: u64,
    ) {
        for (pid_str, rec) in datos {
            let Ok(pid) = pid_str.parse::<PeerId>() else {
                continue;
            };
            let s = self.scores.entry(pid).or_default();
            s.pings_ok = s.pings_ok.max(rec.pings_ok);
            s.pings_fallo = s.pings_fallo.max(rec.pings_fallo);
            s.tx_aceptadas = s.tx_aceptadas.max(rec.tx_aceptadas);
            s.snapshots_servidos = s.snapshots_servidos.max(rec.snapshots_servidos);
            s.strikes_totales = s.strikes_totales.max(rec.strikes_totales);
            if rec.primera_conexion_ms > 0 {
                let edad = Duration::from_millis(now_wall_ms.saturating_sub(rec.primera_conexion_ms));
                match now.checked_sub(edad) {
                    Some(t0) => {
                        let nueva_edad = s.edad_heredada.saturating_add(
                            s.primera_conexion
                                .and_then(|prev| now.checked_duration_since(prev))
                                .unwrap_or(Duration::ZERO),
                        );
                        if s.primera_conexion.is_none() || edad > nueva_edad {
                            s.primera_conexion = Some(t0);
                            s.edad_heredada = Duration::ZERO;
                        }
                    }
                    None => {
                        let current_age = s
                            .primera_conexion
                            .and_then(|t0| now.checked_duration_since(t0))
                            .map(|d| s.edad_heredada.saturating_add(d))
                            .unwrap_or(Duration::ZERO);
                        if s.primera_conexion.is_none() || edad > current_age {
                            s.primera_conexion = Some(now);
                            s.edad_heredada = edad;
                        }
                    }
                }
            }
        }
    }

    fn score_sin_datos(s: &PeerScore) -> bool {
        s.pings_ok == 0
            && s.pings_fallo == 0
            && s.tx_aceptadas == 0
            && s.snapshots_servidos == 0
            && s.strikes_totales == 0
            && s.primera_conexion.is_none()
            && s.edad_heredada.is_zero()
    }
}

/// Score runtime de un peer (node-local, en RAM): strikes + prisiones +
/// métricas de la Prueba de Tiempo (unidad C). La copia persistente
/// (reinicio-proof) es `PeerLocal` en el tree `peers` de Storage.
#[derive(Debug, Default)]
struct PeerScore {
    strikes: VecDeque<(Instant, StrikeKind)>,
    banned_until: Option<Instant>,
    /// Cantidad de bans previos (para el backoff exponencial).
    ban_count: u32,
    // ── Unidad C: Prueba de Tiempo ──
    primera_conexion: Option<Instant>,
    /// Antigüedad acumulada previa al `Instant` de arranque (sobrevive reboots del SO).
    edad_heredada: Duration,
    pings_ok: u64,
    pings_fallo: u64,
    tx_aceptadas: u64,
    snapshots_servidos: u64,
    strikes_totales: u64,
}

/// Baneo node-local. One per node (core loop / wallet actor). Bans viven
/// SOLO en RAM del proceso: al reiniciar el nodo la prisión expira — la
/// "memoria" duradera es trabajo de la Prueba de Tiempo (unidad C).
#[derive(Debug, Default)]
pub struct PeerGuard {
    scores: HashMap<PeerId, PeerScore>,
}

/// UNIX millis de AGORA (wall clock) — el parámetro `now_wall_ms` de
/// export/import. Centralizado para no regar `SystemTime::now()` por el código.
pub fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Duración de un ban con backoff: `min(base * 2^ban_count, BAN_BACKOFF_MAX)`.
/// El primer ban cuesta la base; cada ban repetido del mismo peer dobla la
/// prisión — el spam sostenido no paga.
fn duracion_ban(base: Duration, ban_count_previos: u32) -> Duration {
    let mut d = base;
    for _ in 0..ban_count_previos {
        d = d.saturating_mul(2);
        if d >= BAN_BACKOFF_MAX {
            return BAN_BACKOFF_MAX;
        }
    }
    d
}

impl PeerGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Números de peers con antigüedad conocida (instrumentación/monitores).
    pub fn antiq_count(&self) -> usize {
        self.scores
            .values()
            .filter(|s| s.primera_conexion.is_some())
            .count()
    }

    /// ¿Esta baneado ahora? `Some(restante)` mientras la prisión corra;
    /// `None` cuando expiró (0s restantes = libre).
    pub fn is_banned(&self, peer: &PeerId, now: Instant) -> Option<Duration> {
        self.scores
            .get(peer)
            .and_then(|s| s.banned_until)
            .and_then(|until| until.checked_duration_since(now))
            .filter(|d| !d.is_zero())
    }

    /// Las prisiones del peer (para instrumentación/monitores).
    pub fn ban_count(&self, peer: &PeerId) -> u32 {
        self.scores.get(peer).map(|s| s.ban_count).unwrap_or(0)
    }

    /// Registra un strike y evalúa su régimen. Devuelve `BanCommitido` si
    /// el strike allanó la prisión (el llamador logea WARN con la duración).
    ///
    /// Al banear se LIMPIA toda la ventana de strikes: la "prisión" es el
    /// ban, no una reputación eterna (eso es asunto de la unidad C).
    pub fn note_strike(&mut self, peer: &PeerId, kind: StrikeKind, now: Instant) -> BanOutcome {
        // El margen del veterano (unidad C) se consulta ANTES de tomar el
        // entry mut del peer (borrow rules margen limpio).
        let margen = if kind == StrikeKind::TxInvalid {
            self.margen_strikes(peer, now)
        } else {
            0
        };
        let score = self.scores.entry(peer.clone()).or_default();

        // Ventana rodante: comparar con `saturating_duration_since` (inmune a uptime < 10m).
        while matches!(
            score.strikes.front(),
            Some(&(t, _)) if now.saturating_duration_since(t) > STRIKE_WINDOW
        ) {
            score.strikes.pop_front();
        }
        score.strikes.push_back((now, kind));
        // Prueba de Tiempo (unidad C): todo strike deja marca (débil, para el
        // historial node-local — jamás en consenso).
        score.strikes_totales += 1;

        let ban = match kind {
            StrikeKind::TxInvalid => {
                let limite = LIGHT_STRIKE_LIMIT + margen;
                let lights = score
                    .strikes
                    .iter()
                    .filter(|(_, k)| *k == StrikeKind::TxInvalid)
                    .count() as u32;
                if lights >= limite {
                    BAN_LIGHT_BASE
                } else {
                    return BanOutcome::SinBan;
                }
            }
            StrikeKind::ChainIdSpam => {
                let wet = score
                    .strikes
                    .iter()
                    .filter(|(t, k)| {
                        *k == StrikeKind::ChainIdSpam
                            && now.saturating_duration_since(*t) <= CHAINID_SPAM_WINDOW
                    })
                    .count() as u32;
                if wet > CHAINID_SPAM_LIMIT {
                    BAN_LIGHT_BASE
                } else {
                    return BanOutcome::SinBan;
                }
            }
            StrikeKind::SnapshotInvalido => BAN_GATE_BASE,
            StrikeKind::RequestSpam => BAN_FLOOD_BASE,
        };

        let dur = duracion_ban(ban, score.ban_count);
        score.banned_until = Some(now + dur);
        score.ban_count += 1;
        score.strikes.clear();
        BanOutcome::BanCommitido(dur)
    }

    /// Tope de entradas mantenidas en RAM por `PeerGuard` antes de purgar inactivos/Sybils.
    pub const MAX_PEER_SCORES: usize = 4096;

    /// Higiene del loop de red: limpia bans expirados (`banned_until = None`) y
    /// strikes vencidos SIN borrar `ban_count` ni la reputación histórica (Prueba de
    /// Tiempo / backoff exponencial). Cuando supera `MAX_PEER_SCORES`, expulsa primero
    /// peers sin contribuciones ni bans activos (incluso si completaron handshake) y
    /// acota estrictamente a `MAX_PEER_SCORES`.
    pub fn prune(&mut self, now: Instant) {
        for s in self.scores.values_mut() {
            if matches!(s.banned_until, Some(until) if until <= now) {
                s.banned_until = None;
            }
            while matches!(
                s.strikes.front(),
                Some(&(t, _)) if now.saturating_duration_since(t) > STRIKE_WINDOW
            ) {
                s.strikes.pop_front();
            }
        }
        if self.scores.len() > Self::MAX_PEER_SCORES {
            // Fase 1: purgar handshakes vacíos (Sybils efímeros sin TXs, snapshots, pings ni historial de bans).
            self.scores.retain(|_, s| {
                s.banned_until.is_some()
                    || s.ban_count > 0
                    || !s.strikes.is_empty()
                    || s.tx_aceptadas > 0
                    || s.snapshots_servidos > 0
                    || s.pings_ok > 0
            });
        }
        if self.scores.len() > Self::MAX_PEER_SCORES {
            // Fase 2: si aún excede (p. ej. >4096 peers con pings), evictar los de menor reputación.
            let excess = self.scores.len() - Self::MAX_PEER_SCORES;
            let mut ranked: Vec<(PeerId, (bool, bool, bool, u64, Duration))> = self
                .scores
                .iter()
                .map(|(pid, s)| {
                    let active_ban = s.banned_until.is_some() || !s.strikes.is_empty();
                    let repeat_offender = s.ban_count > 0;
                    let useful = s.tx_aceptadas > 0 || s.snapshots_servidos > 0;
                    let score = s
                        .tx_aceptadas
                        .saturating_add(s.snapshots_servidos)
                        .saturating_add(s.pings_ok);
                    let age = s
                        .primera_conexion
                        .and_then(|t0| now.checked_duration_since(t0))
                        .map(|d| s.edad_heredada.saturating_add(d))
                        .unwrap_or(Duration::ZERO);
                    (*pid, (active_ban, repeat_offender, useful, score, age))
                })
                .collect();
            ranked.sort_by_key(|(_, rank)| *rank);
            for (pid, _) in ranked.into_iter().take(excess) {
                self.scores.remove(&pid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    fn peer(seed: u8) -> PeerId {
        let key = libp2p::identity::Keypair::ed25519_from_bytes([seed; 32]).unwrap();
        PeerId::from(key.public())
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn test_strikes_leves_no_banean_antes_del_limite() {
        let mut guard = PeerGuard::new();
        let p = peer(1);
        let now = t0();
        for _ in 0..LIGHT_STRIKE_LIMIT - 1 {
            assert_eq!(
                guard.note_strike(&p, StrikeKind::TxInvalid, now),
                BanOutcome::SinBan,
                "2 leves no devem banear nada"
            );
        }
        assert_eq!(guard.is_banned(&p, now), None, "sin 3 leves no hay prisión");
    }

    #[test]
    fn test_3_leves_en_ventana_banean_30min() {
        let mut guard = PeerGuard::new();
        let p = peer(2);
        let now = t0();
        guard.note_strike(&p, StrikeKind::TxInvalid, now);
        guard.note_strike(&p, StrikeKind::TxInvalid, now + Duration::from_secs(60));
        assert_eq!(
            guard.note_strike(&p, StrikeKind::TxInvalid, now + Duration::from_secs(120)),
            BanOutcome::BanCommitido(BAN_LIGHT_BASE),
            "3 leves en <10min ⇒ ban 30min (sin backoff todavía)"
        );
        assert_eq!(
            guard.is_banned(&p, now + Duration::from_secs(120)),
            Some(BAN_LIGHT_BASE),
            "ban al instante del 3º strike (sin backoff todavía)"
        );
    }

    #[test]
    fn test_ventana_rodante_evita_el_ban() {
        // 1 leve, y sus 2 siguientes llegan FUERA de la ventana de 10 min:
        // solo valen esos 2 (el primero expiró) → no ban.
        let mut guard = PeerGuard::new();
        let p = peer(3);
        let now = t0();
        guard.note_strike(&p, StrikeKind::TxInvalid, now);
        let tardio = now + STRIKE_WINDOW + Duration::from_secs(1);
        guard.note_strike(&p, StrikeKind::TxInvalid, tardio);
        assert_eq!(
            guard.note_strike(&p, StrikeKind::TxInvalid, tardio + Duration::from_secs(1)),
            BanOutcome::SinBan,
            "los strikes vencidos no cuentan (ventana rodante)"
        );
    }

    #[test]
    fn test_snapshot_invalido_ban_inmediato_60min() {
        // Decisión §5-①: adulterio del estado ⇒ 1 strike grave basta.
        let mut guard = PeerGuard::new();
        let p = peer(4);
        let now = t0();
        assert_eq!(
            guard.note_strike(&p, StrikeKind::SnapshotInvalido, now),
            BanOutcome::BanCommitido(BAN_GATE_BASE)
        );
        assert!(guard.is_banned(&p, now).is_some());
    }

    #[test]
    fn test_flood_ban_inmediato_10min() {
        let mut guard = PeerGuard::new();
        let p = peer(5);
        let now = t0();
        assert_eq!(
            guard.note_strike(&p, StrikeKind::RequestSpam, now),
            BanOutcome::BanCommitido(BAN_FLOOD_BASE)
        );
    }

    #[test]
    fn test_ban_expira() {
        let mut guard = PeerGuard::new();
        let p = peer(6);
        let now = t0();
        guard.note_strike(&p, StrikeKind::RequestSpam, now);
        assert!(guard.is_banned(&p, now).is_some());
        assert!(
            guard.is_banned(&p, now + BAN_FLOOD_BASE).is_none(),
            "vencida la prisión, el peer recupera el derecho al dial"
        );
    }

    #[test]
    fn test_backoff_exponencial_para_bans_repetidos() {
        // flood 10min → vence → re- strike → 20min → ... cap 24h.
        let mut guard = PeerGuard::new();
        let p = peer(7);
        let mut now = t0();
        assert_eq!(
            guard.note_strike(&p, StrikeKind::RequestSpam, now),
            BanOutcome::BanCommitido(BAN_FLOOD_BASE)
        );
        now = now + BAN_FLOOD_BASE + Duration::from_secs(1);
        assert_eq!(
            guard.note_strike(&p, StrikeKind::RequestSpam, now),
            BanOutcome::BanCommitido(BAN_FLOOD_BASE * 2),
            "el 2º ban dobla la prisión"
        );
        now = now + BAN_FLOOD_BASE * 3 + Duration::from_secs(1);
        assert_eq!(
            guard.note_strike(&p, StrikeKind::RequestSpam, now),
            BanOutcome::BanCommitido(BAN_FLOOD_BASE * 4)
        );
        // La prisión jamás supera las 24h de backoff (tope absoluto).
        assert_eq!(duracion_ban(BAN_FLOOD_BASE, 40), BAN_BACKOFF_MAX);
        assert_eq!(duracion_ban(BAN_GATE_BASE, 6), BAN_BACKOFF_MAX);
    }

    #[test]
    fn test_ban_limpia_strikes_previos() {
        // Al banear se limpia la ventana penitencial que quedó: tras terminar
        // la prisión, el peer arranca 0 strikes (la reputación eterna es
        // asunto de C, no del guard).
        let mut guard = PeerGuard::new();
        let p = peer(9);
        let now = t0();
        guard.note_strike(&p, StrikeKind::TxInvalid, now);
        guard.note_strike(&p, StrikeKind::TxInvalid, now);
        guard.note_strike(&p, StrikeKind::TxInvalid, now); // ban aquí limpia los 3
        assert_eq!(guard.ban_count(&p), 1);
        // pasado el ban, un par nuevo de leves NO debe banear de nuevo al instante
        let ahora2 = now + BAN_LIGHT_BASE + Duration::from_secs(1);
        guard.note_strike(&p, StrikeKind::TxInvalid, ahora2);
        guard.note_strike(&p, StrikeKind::TxInvalid, ahora2);
        assert_eq!(
            guard.note_strike(&p, StrikeKind::TxInvalid, ahora2),
            BanOutcome::BanCommitido(BAN_LIGHT_BASE * 2),
            "sí: al ser el 2º ban en la historia del peer, la prisión doblaría (backoff)"
        );
    }

    #[test]
    fn test_chainid_spam_rate_based() {
        // Leves "gratis": 20 en ventana no banean; el 21º dentro de 5 min sí.
        let mut guard = PeerGuard::new();
        let p = peer(10);
        let now = t0();
        for i in 0..CHAINID_SPAM_LIMIT {
            assert_eq!(
                guard.note_strike(
                    &p,
                    StrikeKind::ChainIdSpam,
                    now + Duration::from_millis(i as u64)
                ),
                BanOutcome::SinBan,
                "chain_id leves NO banean por cantidad mínima (versión antiquísima honesta)"
            );
        }
        assert_eq!(
            guard.note_strike(
                &p,
                StrikeKind::ChainIdSpam,
                now + Duration::from_millis(CHAINID_SPAM_LIMIT as u64)
            ),
            BanOutcome::BanCommitido(BAN_LIGHT_BASE),
            "21º chain_id en 5 min es saturación: spam grave"
        );
    }

    #[test]
    fn test_leves_de_chainid_ignoran_el_reto_de_txinv() {
        // Regla clave del equilibrio honesto: ChainIdSpam NO cuenta en la
        // canasta `TxInvalid` de 3 leves — neto de version skew.
        let mut guard = PeerGuard::new();
        let p = peer(11);
        let now = t0();
        guard.note_strike(&p, StrikeKind::ChainIdSpam, now);
        guard.note_strike(&p, StrikeKind::ChainIdSpam, now);
        assert_eq!(
            guard.note_strike(&p, StrikeKind::TxInvalid, now),
            BanOutcome::SinBan,
            "un único TxInvalid con spam de versión no debe banear"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Unidad C: Prueba de Tiempo (métricas node-local)
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn test_antiguedad_marca_la_primer_conexion() {
        let mut guard = PeerGuard::new();
        let p = peer(20);
        let now = t0();
        assert!(
            guard.antiguedad(&p, now).is_none(),
            "sin conexión no hay antigüedad"
        );
        guard.note_conexion(&p, now);
        assert_eq!(
            guard.antiguedad(&p, now + Duration::from_secs(3600)),
            Some(Duration::from_secs(3600)),
            "antigüedad = tiempo desde la PRIMERA conexión"
        );
    }

    #[test]
    fn test_reconexiones_no_retroceden_la_antiguedad() {
        let mut guard = PeerGuard::new();
        let p = peer(21);
        let now = t0();
        guard.note_conexion(&p, now);
        guard.note_conexion(&p, now + Duration::from_secs(40 * 60));
        assert_eq!(
            guard.antiguedad(&p, now + Duration::from_secs(40 * 60)),
            Some(Duration::from_secs(40 * 60)),
            "la re-conexión NO debe resetear la antigüedad (es de SIEMPRE)"
        );
    }

    #[test]
    fn test_uptime_ratio_matematica() {
        let mut guard = PeerGuard::new();
        let p = peer(22);
        assert!(guard.uptime_ratio(&p).is_none(), "sin pings no hay ratio");
        for _ in 0..19 {
            guard.note_ping(&p, true);
        }
        guard.note_ping(&p, false);
        let ratio = guard.uptime_ratio(&p).unwrap();
        assert!(
            (ratio - 0.95).abs() < 1e-9,
            "19 OK / 20 totales = 0.95, salió {ratio}"
        );
    }

    #[test]
    fn test_margen_solo_para_el_veterano() {
        // Veteranía: antigüedad + uptime ≥ 99% + datos de ping suficientes
        // ⇒ 1 strike de margen (el 3º leve no banea todavía).
        let mut guard = PeerGuard::new();
        let p = peer(23);
        let ahora = t0();
        let pasado = ahora - MARGEN_ANTIGUEDAD_MIN - Duration::from_secs(60);
        guard.note_conexion(&p, pasado);
        for _ in 0..100 {
            guard.note_ping(&p, true);
        }
        assert_eq!(
            guard.margen_strikes(&p, ahora),
            1,
            "veterano confiable ⇒ margen 1"
        );
        for _ in 0..LIGHT_STRIKE_LIMIT - 1 {
            guard.note_strike(&p, StrikeKind::TxInvalid, ahora);
        }
        assert_eq!(
            guard.note_strike(&p, StrikeKind::TxInvalid, ahora + Duration::from_secs(60)),
            BanOutcome::SinBan,
            "con margen 1, 4 leves son los que banean (no 3)"
        );
    }

    #[test]
    fn test_el_veterano_no_escapa_lo_grave() {
        // El margen SOLO enmascara leves de TxInvalid: gate/flood banean igual
        // a un veterano (ser viejo no te hace adulterador).
        let mut guard = PeerGuard::new();
        let p = peer(24);
        let ahora = t0();
        guard.note_conexion(&p, ahora - MARGEN_ANTIGUEDAD_MIN);
        for _ in 0..100 {
            guard.note_ping(&p, true);
        }
        assert_eq!(guard.margen_strikes(&p, ahora), 1);
        assert_eq!(
            guard.note_strike(&p, StrikeKind::SnapshotInvalido, ahora),
            BanOutcome::BanCommitido(BAN_GATE_BASE),
            "el ban de gate es INMEDIATO aun con margen"
        );
    }

    #[test]
    fn test_export_import_sobrevive_el_reinicio() {
        // La promesa de la unidad C: cortar el nodo, abrir de nuevo, y la
        // antigüedad sigue siendo LA MISMA (persistencia node-local).
        let mut guard = PeerGuard::new();
        let p = peer(25);
        let ahora = t0();
        guard.note_conexion(&p, ahora - Duration::from_secs(3600)); // 1h antiguo
        guard.note_ping(&p, true);
        guard.note_tx_aceptada(&p);
        guard.note_snapshot_servido(&p);

        let wall_ms = unix_millis_now();
        let registros = guard.export_records(ahora, wall_ms);
        let rec = registros.get(&p.to_string()).expect("peer exportado");
        assert_eq!(rec.pings_ok, 1);
        assert_eq!(rec.tx_aceptadas, 1);
        assert_eq!(rec.snapshots_servidos, 1);
        let edad_exportada = wall_ms - rec.primera_conexion_ms;
        assert!(
            (edad_exportada as i64 - 3600_000).abs() < 60_000,
            "la edad exportada debe ser ~1h, salió {edad_exportada} ms"
        );

        // "Reinicio": guard nuevo + import.
        let mut renacido = PeerGuard::new();
        renacido.import_records(registros, ahora, wall_ms);
        assert_eq!(
            renacido.antiguedad(&p, ahora),
            Some(Duration::from_secs(3600)),
            "la antigüedad sobrevive al reinicio (es de SIEMPRE)"
        );
    }

    #[test]
    fn test_prioridad_dial_veteranos_primeros() {
        let mut guard = PeerGuard::new();
        let nuevo = peer(26);
        let vetusto = peer(27);
        let desconocido = peer(28);
        let ahora = t0();
        guard.note_conexion(&nuevo, ahora - Duration::from_secs(60)); // 1 min
        guard.note_conexion(&vetusto, ahora - Duration::from_secs(3600)); // 1h
        let orden = guard.prioridad_dial(&[nuevo, desconocido, vetusto], ahora);
        assert_eq!(orden[0], vetusto, "el más antiguo diala primero");
        assert_eq!(orden[1], nuevo);
        assert_eq!(orden[2], desconocido, "sin historia va al final");
    }

    #[test]
    fn test_prune_conserva_ban_count_y_antiguedad_tras_expirar() {
        let mut guard = PeerGuard::new();
        let p = peer(29);
        let ahora = t0();
        guard.note_conexion(&p, ahora);
        guard.note_tx_aceptada(&p);

        // Primer ban: 60 min (SnapshotInvalido) -> ban_count = 1
        assert_eq!(
            guard.note_strike(&p, StrikeKind::SnapshotInvalido, ahora),
            BanOutcome::BanCommitido(BAN_GATE_BASE)
        );
        assert_eq!(guard.ban_count(&p), 1);

        // Expira el ban y corre prune()
        let despues = ahora + BAN_GATE_BASE + Duration::from_secs(1);
        guard.prune(despues);
        assert_eq!(guard.is_banned(&p, despues), None);
        assert_eq!(guard.ban_count(&p), 1, "prune no debe borrar ban_count");
        assert!(guard.antiguedad(&p, despues).is_some(), "antigüedad intacta");

        // Segundo ban: debe aplicar backoff exponencial (2 * BAN_GATE_BASE = 2h)
        assert_eq!(
            guard.note_strike(&p, StrikeKind::SnapshotInvalido, despues),
            BanOutcome::BanCommitido(BAN_GATE_BASE * 2)
        );
        assert_eq!(guard.ban_count(&p), 2);
    }

    #[test]
    fn test_ckpt54_import_conserva_veterania_mayor_al_uptime_y_prune_acota_sybils() {
        let mut guard = PeerGuard::new();
        let vet = peer(30);
        let ahora = t0();
        let cien_anos_ms = 100u64 * 365 * 24 * 3600 * 1000;
        let now_wall_ms = u64::MAX / 2;

        let mut datos = HashMap::new();
        datos.insert(
            vet.to_string(),
            PeerLocal {
                primera_conexion_ms: now_wall_ms - cien_anos_ms,
                pings_ok: 100,
                pings_fallo: 0,
                tx_aceptadas: 5,
                snapshots_servidos: 5,
                strikes_totales: 0,
            },
        );
        // Incluso cuando now.checked_sub(edad) falla por uptime corto tras reboot,
        // `edad_heredada` debe preservar la veteranía intacta.
        guard.import_records(datos, ahora, now_wall_ms);
        let antig = guard
            .antiguedad(&vet, ahora + Duration::from_secs(10))
            .expect("veterano importado debe tener antigüedad");
        assert!(antig >= Duration::from_millis(cien_anos_ms));
        assert_eq!(guard.margen_strikes(&vet, ahora + Duration::from_secs(10)), 1);

        // Además, los handshakes Sybil sin historial deben purgarse al pasar MAX_PEER_SCORES.
        for _ in 0..(PeerGuard::MAX_PEER_SCORES + 100) {
            guard.note_conexion(&PeerId::random(), ahora);
        }
        guard.prune(ahora);
        assert!(guard.scores.len() <= PeerGuard::MAX_PEER_SCORES);
        assert!(
            guard.antiguedad(&vet, ahora + Duration::from_secs(10)).is_some(),
            "el veterano útil no debe ser expulsado por handshakes Sybil"
        );
    }
}
