//! État du PC consulté avant un changement automatique : alimentation, plein écran, connexion limitée.

use crate::config::Config;
use windows::Win32::Networking::NetworkListManager::{INetworkCostManager, NetworkListManager};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};

/// Le PC fonctionne sur batterie (faux sur un PC fixe ou en cas de doute).
pub fn on_battery() -> bool {
    let mut st = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut st).is_ok() && st.ACLineStatus == 0 }
}

/// Une application occupe tout l'écran (jeu, vidéo, présentation).
pub fn fullscreen_busy() -> bool {
    match unsafe { SHQueryUserNotificationState() } {
        Ok(s) => s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE,
        Err(_) => false,
    }
}

/// Coût de la connexion Internet active (INetworkCostManager::GetCost).
pub fn connection_cost() -> windows::core::Result<u32> {
    unsafe {
        let m: INetworkCostManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
        let mut cost = 0u32;
        m.GetCost(&mut cost, std::ptr::null())?;
        Ok(cost)
    }
}

/// Connexion limitée ou facturée à l'usage (partage de connexion, forfait de données,
/// itinérance). Faux si l'information est indisponible.
pub fn metered_connection() -> bool {
    // NLM_CONNECTION_COST_FIXED (2), _VARIABLE (4), _OVERDATALIMIT (0x10000), _ROAMING (0x40000)
    matches!(connection_cost(), Ok(c) if c & (0x2 | 0x4 | 0x10000 | 0x40000) != 0)
}

/// Pourquoi un changement automatique doit attendre (None = rien ne s'y oppose).
/// hours=false ignore la plage horaire (préchargement).
pub fn blocked(cfg: &Config, hours: bool) -> Option<&'static str> {
    if hours && !cfg.active_at(&crate::util::now_local()) {
        Some("hors plage horaire")
    } else if cfg.pause_on_battery && on_battery() {
        Some("sur batterie")
    } else if cfg.pause_on_fullscreen && fullscreen_busy() {
        Some("application en plein écran")
    } else if cfg.pause_on_metered && metered_connection() {
        Some("connexion limitée")
    } else {
        None
    }
}
