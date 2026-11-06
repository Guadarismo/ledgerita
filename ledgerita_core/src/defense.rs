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

#[derive(Debug, Default)]
struct PeerScore {
    strikes: VecDeque<(Instant, StrikeKind)>,
    banned_until: Option<Instant>,
    /// Cantidad de bans previos (para el backoff exponencial).
    ban_count: u32,
}

/// Baneo node-local. One per node (core loop / wallet actor). Bans viven
/// SOLO en RAM del proceso: al reiniciar el nodo la prisión expira — la
/// "memoria" duradera es trabajo de la Prueba de Tiempo (unidad C).
#[derive(Debug, Default)]
pub struct PeerGuard {
    scores: HashMap<PeerId, PeerScore>,
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
        let score = self.scores.entry(peer.clone()).or_default();

        // Ventana rodante: caen los strikes viejos ANTES de evaluar.
        let cuts = now.checked_sub(STRIKE_WINDOW).unwrap_or(now);
        while matches!(score.strikes.front(), Some(&(t, _)) if t < cuts) {
            score.strikes.pop_front();
        }
        score.strikes.push_back((now, kind));

        let ban = match kind {
            StrikeKind::TxInvalid => {
                let lights = score
                    .strikes
                    .iter()
                    .filter(|(_, k)| *k == StrikeKind::TxInvalid)
                    .count() as u32;
                if lights >= LIGHT_STRIKE_LIMIT {
                    BAN_LIGHT_BASE
                } else {
                    return BanOutcome::SinBan;
                }
            }
            StrikeKind::ChainIdSpam => {
                let spam_min = now
                    .checked_sub(CHAINID_SPAM_WINDOW)
                    .unwrap_or(now);
                let wet = score
                    .strikes
                    .iter()
                    .filter(|(t, k)| {
                        *k == StrikeKind::ChainIdSpam && *t >= spam_min
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

    /// Higiene del loop de red: descarta las prisiones vencidas y las
    /// ventanas de strikes muertas (llamar de vez en cuando; en el tick de
    /// 30 s es suficiente).
    pub fn prune(&mut self, now: Instant) {
        let cuts = now.checked_sub(STRIKE_WINDOW).unwrap_or(now);
        self.scores.retain(|_, s| {
            !matches!(s.banned_until, Some(until) if until <= now)
        });
        for (_, s) in self.scores.iter_mut() {
            while matches!(s.strikes.front(), Some(&(t, _)) if t < cuts) {
                s.strikes.pop_front();
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
        assert_eq!(
            guard.is_banned(&p, now),
            None,
            "sin 3 leves no hay prisión"
        );
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
                guard.note_strike(&p, StrikeKind::ChainIdSpam, now + Duration::from_millis(i as u64)),
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
}
