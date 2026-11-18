//! Un fond d'écran par écran : interface COM IDesktopWallpaper (Windows 8+).

use crate::core::Monitor;
use crate::logf;
use crate::render::Screen;
use crate::util::{Error, Res};
use crate::win::platform::{set_background_type_picture, wide};
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::UI::Shell::{
    DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL, DWPOS_FIT, DWPOS_SPAN, DWPOS_STRETCH,
    DWPOS_TILE,
};

fn desktop_wallpaper() -> windows::core::Result<IDesktopWallpaper> {
    unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
}

fn work_area_inset(rc: &RECT) -> i32 {
    unsafe {
        let h = MonitorFromRect(rc, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if h.is_invalid() || !GetMonitorInfoW(h, &mut mi).as_bool() {
            return 0;
        }
        let inset = mi.rcMonitor.bottom - mi.rcWork.bottom;
        if inset < 0 || inset > (mi.rcMonitor.bottom - mi.rcMonitor.top) / 4 {
            0
        } else {
            inset
        }
    }
}

/// Écrans actifs, l'écran principal en premier puis de gauche à droite.
pub fn list_monitors() -> Vec<Monitor> {
    let list = (|| -> windows::core::Result<Vec<(Monitor, i32, bool)>> {
        let dw = desktop_wallpaper()?;
        let mut list = vec![];
        unsafe {
            for i in 0..dw.GetMonitorDevicePathCount()? {
                let Ok(p) = dw.GetMonitorDevicePathAt(i) else { continue };
                let id = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                let wid = wide(&id);
                let Ok(rc) = dw.GetMonitorRECT(PCWSTR(wid.as_ptr())) else {
                    continue;
                }; // écran inactif
                let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
                if id.is_empty() || w <= 0 || h <= 0 {
                    continue; // écran déconnecté
                }
                let screen = Screen {
                    w,
                    h,
                    bottom_inset: work_area_inset(&rc),
                };
                list.push((Monitor { id, screen }, rc.left, rc.left == 0 && rc.top == 0));
            }
        }
        Ok(list)
    })();
    match list {
        Ok(mut l) => {
            l.sort_by_key(|(_, left, primary)| (!*primary, *left));
            l.into_iter().map(|x| x.0).collect()
        }
        Err(e) => {
            logf!("énumération des écrans : {e}");
            vec![]
        }
    }
}

fn position(style: &str) -> DESKTOP_WALLPAPER_POSITION {
    match style {
        "center" => DWPOS_CENTER,
        "tile" => DWPOS_TILE,
        "stretch" => DWPOS_STRETCH,
        "fit" => DWPOS_FIT,
        "span" => DWPOS_SPAN,
        _ => DWPOS_FILL,
    }
}

/// Applique une image par écran (chemin de périphérique → fichier).
pub fn set_monitor_wallpapers(paths: &[(String, PathBuf)], style: &str) -> Res<()> {
    set_background_type_picture();
    let res = (|| -> windows::core::Result<()> {
        let dw = desktop_wallpaper()?;
        unsafe {
            if let Err(e) = dw.SetPosition(position(style)) {
                logf!("ajustement du fond d'écran (SetPosition) : {e}");
            }
            for (id, path) in paths {
                let (wid, wpath) = (wide(id), wide(&path.to_string_lossy()));
                dw.SetWallpaper(PCWSTR(wid.as_ptr()), PCWSTR(wpath.as_ptr()))?;
            }
        }
        Ok(())
    })();
    res.map_err(|e| Error::Msg(format!("IDesktopWallpaper : {e}")))
}
