//! Proxy de restricciones entre la UI y librespot.
//!
//! Dos trabajos:
//! 1. Filtrar comandos que violen las restricciones de la cuenta ANTES de que
//!    lleguen a Spirc, y emitir `spotify://restriction` con un mensaje claro.
//! 2. Documentar con honestidad lo que librespot NO puede hacer (ver §6 del doc).
//!
//! Reglas implementadas (única regla Free documentada por Spotify que aplicamos):
//! - Cuentas restringidas (Free/desconocido): máximo 6 saltos por hora deslizante.
//!   `prev` no consume (normalmente reinicia el tema actual).
//! - Reproducción bajo demanda de un URI concreto: denegada en restringidas
//!   (la app oficial solo permite aleatorio con anuncios en Free).
//! - Premium: pass-through (el servidor de Spotify aplica sus propias reglas).
//!
//! NOTA DE HONESTIDAD: hoy este filtro casi nunca llega a actuar, porque el gate
//! de `account_gate` impide conectar con cuentas no-Premium (librespot hace
//! `exit(1)` en `check_catalogue` con cuentas Free: mataría el proceso). La capa
//! existe como defensa en profundidad y como contrato de mensajes testeable.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// Límite de saltos de la oferta gratuita (ventana deslizante de 1 hora).
pub const FREE_SKIPS_PER_HOUR: u32 = 6;
pub const SKIP_WINDOW_SECS: u64 = 3600;

/// Nivel de cuenta conocido por la app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountTier {
    /// Premium verificado (gate de /v1/me o tier persistido tras verificar).
    Premium,
    /// Free o desconocido: se aplica la política estricta.
    Restricted,
}

impl AccountTier {
    pub fn from_product(product: Option<&str>) -> Self {
        match product.map(|p| p.to_ascii_lowercase()) {
            Some(p) if p == "premium" => AccountTier::Premium,
            _ => AccountTier::Restricted,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AccountTier::Premium => "premium",
            AccountTier::Restricted => "restricted",
        }
    }
}

/// Denegación de un comando por política (serializable para IPC).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PolicyDenial {
    /// "skip-budget" | "ondemand-forbidden" | "disconnected" | "internal".
    pub code: &'static str,
    pub message: String,
    pub skips_used: u32,
    pub skips_max: u32,
    pub retry_after_secs: u64,
}

impl PolicyDenial {
    pub fn disconnected() -> Self {
        Self {
            code: "disconnected",
            message: "Spotify no está conectado".into(),
            skips_used: 0,
            skips_max: FREE_SKIPS_PER_HOUR,
            retry_after_secs: 0,
        }
    }

    pub fn internal() -> Self {
        Self {
            code: "internal",
            message: "error interno de sincronización del motor Spotify".into(),
            skips_used: 0,
            skips_max: FREE_SKIPS_PER_HOUR,
            retry_after_secs: 0,
        }
    }
}

/// Evento IPC `spotify://restriction` para mostrar el motivo en la UI.
#[derive(Debug, Clone, Serialize)]
pub struct RestrictionEvent {
    pub tier: AccountTier,
    pub code: &'static str,
    pub message: String,
    pub skips_used: u32,
    pub skips_max: u32,
    pub retry_after_secs: u64,
}

/// Estado del presupuesto de saltos para la UI (`spotify_policy_status`).
#[derive(Debug, Clone, Serialize)]
pub struct BudgetSnapshot {
    pub tier: AccountTier,
    /// librespot no inserta anuncios: siempre false (ver limitaciones).
    pub ads_supported: bool,
    pub skips_used: u32,
    pub skips_max: u32,
    pub retry_after_secs: u64,
}

pub fn emit_restriction(app: &AppHandle, tier: AccountTier, d: &PolicyDenial) {
    app.emit(
        "spotify://restriction",
        RestrictionEvent {
            tier,
            code: d.code,
            message: d.message.clone(),
            skips_used: d.skips_used,
            skips_max: d.skips_max,
            retry_after_secs: d.retry_after_secs,
        },
    )
    .ok();
}

fn system_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Presupuesto de saltos con ventana deslizante. El reloj es inyectable
/// (cada test usa el suyo: un global compartido mete carreras en paralelo).
pub struct SkipBudget {
    skips: VecDeque<u64>,
    now_fn: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl SkipBudget {
    pub fn new() -> Self {
        Self { skips: VecDeque::new(), now_fn: std::sync::Arc::new(system_now_secs) }
    }

    pub fn with_clock(now_fn: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { skips: VecDeque::new(), now_fn }
    }

    fn now(&self) -> u64 {
        (self.now_fn)()
    }

    fn prune(&mut self) {
        let now = self.now();
        let cutoff = now.saturating_sub(SKIP_WINDOW_SECS);
        while self.skips.front().is_some_and(|&t| t <= cutoff) {
            self.skips.pop_front();
        }
    }

    pub fn used(&mut self) -> u32 {
        self.prune();
        self.skips.len().min(u32::MAX as usize) as u32
    }

    /// Segundos hasta que se libera un salto (0 si hay presupuesto).
    pub fn retry_after(&mut self) -> u64 {
        self.prune();
        if (self.skips.len() as u32) < FREE_SKIPS_PER_HOUR {
            return 0;
        }
        match self.skips.front() {
            Some(&oldest) => oldest + SKIP_WINDOW_SECS - self.now().min(oldest + SKIP_WINDOW_SECS),
            None => 0,
        }
    }

    /// Consume un salto o indica la espera. Devuelve restantes tras consumir.
    pub fn try_consume(&mut self) -> Result<u32, u64> {
        self.prune();
        if (self.skips.len() as u32) >= FREE_SKIPS_PER_HOUR {
            return Err(self.retry_after());
        }
        self.skips.push_back(self.now());
        Ok(FREE_SKIPS_PER_HOUR - self.skips.len() as u32)
    }
}

impl Default for SkipBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// Política viva del motor Spotify (tras `Mutex` en el handle).
pub struct SpotifyPolicy {
    tier: AccountTier,
    budget: SkipBudget,
}

impl SpotifyPolicy {
    pub fn new(tier: AccountTier) -> Self {
        Self { tier, budget: SkipBudget::new() }
    }

    pub fn with_clock(tier: AccountTier, clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { tier, budget: SkipBudget::with_clock(clock) }
    }

    pub fn tier(&self) -> AccountTier {
        self.tier
    }

    /// ¿Puede saltar al siguiente? Premium siempre; restringidas con presupuesto.
    pub fn check_skip(&mut self) -> Result<(), PolicyDenial> {
        if self.tier == AccountTier::Premium {
            return Ok(());
        }
        match self.budget.try_consume() {
            Ok(_) => Ok(()),
            Err(wait) => {
                let mins = wait.div_ceil(60).max(1);
                Err(PolicyDenial {
                    code: "skip-budget",
                    message: format!(
                        "Has agotado los saltos de la cuenta gratuita ({} por hora). \
                         Podrás saltar de nuevo en ~{} min. Con Spotify Premium no hay límite.",
                        FREE_SKIPS_PER_HOUR, mins
                    ),
                    skips_used: self.budget.used(),
                    skips_max: FREE_SKIPS_PER_HOUR,
                    retry_after_secs: wait,
                })
            }
        }
    }

    /// ¿Puede reproducir un URI concreto bajo demanda?
    pub fn check_play_ondemand(&self) -> Result<(), PolicyDenial> {
        if self.tier == AccountTier::Premium {
            return Ok(());
        }
        Err(PolicyDenial {
            code: "ondemand-forbidden",
            message: "La reproducción bajo demanda no está disponible en cuentas \
                      gratuitas (la app oficial solo permite modo aleatorio con anuncios). \
                      librespot exige Spotify Premium y además no reproduce anuncios."
                .into(),
            skips_used: 0,
            skips_max: FREE_SKIPS_PER_HOUR,
            retry_after_secs: 0,
        })
    }

    pub fn snapshot(&mut self) -> BudgetSnapshot {
        let used = self.budget.used();
        let retry = self.budget.retry_after();
        BudgetSnapshot {
            tier: self.tier,
            ads_supported: false,
            skips_used: used,
            skips_max: FREE_SKIPS_PER_HOUR,
            retry_after_secs: retry,
        }
    }
}

/// Guarda la política compartida (el handle la expone para router e IPC).
pub type SharedPolicy = std::sync::Arc<Mutex<SpotifyPolicy>>;

pub fn shared_policy(tier: AccountTier) -> SharedPolicy {
    std::sync::Arc::new(Mutex::new(SpotifyPolicy::new(tier)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Reloj propio por test (un global compartido mete carreras en paralelo).
    fn clock_at(base: u64) -> (std::sync::Arc<AtomicU64>, std::sync::Arc<dyn Fn() -> u64 + Send + Sync>) {
        let t = std::sync::Arc::new(AtomicU64::new(base));
        let t2 = t.clone();
        (t, std::sync::Arc::new(move || t2.load(Ordering::SeqCst)))
    }

    #[test]
    fn presupuesto_6_por_hora_con_ventana_deslizante() {
        let (now, clock) = clock_at(1_700_000_000);
        let mut p = SpotifyPolicy::with_clock(AccountTier::Restricted, clock);
        // 6 saltos permitidos…
        for i in 1..=6 {
            assert!(p.check_skip().is_ok(), "salto {i} debe pasar");
        }
        // …el 7º se deniega con espera ≈ 1h y mensaje útil.
        let d = p.check_skip().expect_err("el 7º salto debe denegarse");
        assert_eq!(d.code, "skip-budget");
        assert_eq!((d.skips_used, d.skips_max), (6, 6));
        assert!(d.retry_after_secs > 3500 && d.retry_after_secs <= 3600);
        assert!(d.message.contains("6 por hora"));
        // Avanza 1h: el presupuesto se renueva (ventana deslizante).
        now.fetch_add(3601, Ordering::SeqCst);
        assert!(p.check_skip().is_ok(), "tras 1h debe renovarse");
    }

    #[test]
    fn premium_no_tiene_limite() {
        let (_now, clock) = clock_at(1_800_000_000);
        let mut p = SpotifyPolicy::with_clock(AccountTier::Premium, clock);
        for _ in 0..100 {
            assert!(p.check_skip().is_ok());
        }
        assert!(p.check_play_ondemand().is_ok());
    }

    #[test]
    fn bajo_demanda_denegado_en_restringidas() {
        let p = SpotifyPolicy::new(AccountTier::Restricted);
        let d = p.check_play_ondemand().expect_err("Free no tiene bajo demanda");
        assert_eq!(d.code, "ondemand-forbidden");
        assert!(d.message.contains("Premium"));
    }

    #[test]
    fn tier_se_deriva_del_producto() {
        assert_eq!(AccountTier::from_product(Some("premium")), AccountTier::Premium);
        assert_eq!(AccountTier::from_product(Some("Premium")), AccountTier::Premium);
        assert_eq!(AccountTier::from_product(Some("free")), AccountTier::Restricted);
        assert_eq!(AccountTier::from_product(Some("open")), AccountTier::Restricted);
        assert_eq!(AccountTier::from_product(None), AccountTier::Restricted);
    }

    #[test]
    fn evento_ipc_serializa_para_la_ui() {
        let ev = RestrictionEvent {
            tier: AccountTier::Restricted,
            code: "skip-budget",
            message: "límite".into(),
            skips_used: 6,
            skips_max: 6,
            retry_after_secs: 3599,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"tier\":\"restricted\""));
        assert!(json.contains("\"code\":\"skip-budget\""));
        assert!(json.contains("\"skips_used\":6"));
    }

    #[test]
    fn snapshot_informa_a_la_ui() {
        let (_now, clock) = clock_at(1_900_000_000);
        let mut p = SpotifyPolicy::with_clock(AccountTier::Restricted, clock);
        p.check_skip().ok();
        p.check_skip().ok();
        let s = p.snapshot();
        assert_eq!((s.skips_used, s.skips_max), (2, 6));
        assert!(!s.ads_supported, "librespot no gestiona anuncios");
        assert_eq!(s.tier, AccountTier::Restricted);
    }
}
