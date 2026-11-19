//! Fenêtre de réglages, dans le style d'Immich : barre latérale, cartes arrondies, interrupteurs,
//! boutons en pastille, grille d'albums avec leurs vignettes de couverture, thème clair ou sombre
//! selon Windows.
//!
//! Tout est dessiné par l'appli (GDI+ pour les formes antialiasées, GDI pour le texte ClearType),
//! sans bibliothèque d'interface. Les seuls contrôles Windows sont les zones de saisie (EDIT) et
//! des boutons « owner-draw », qui gardent le clavier, le focus et l'accessibilité de Windows.
//!
//! La fenêtre vit sur le thread de l'interface (celui de l'icône de la barre système) : la boucle
//! de messages de l'appli lui passe d'abord chaque message (`pre_translate`) pour la navigation au
//! clavier. Le test de connexion, les vignettes et « Image suivante » tournent dans des threads ;
//! leur résultat revient par un message posté à la fenêtre. Aucun port n'est ouvert, aucun
//! navigateur n'est lancé.

use crate::config::{normalize_server_url, AlbumRule, Config};
use crate::immich::AlbumInfo;
use crate::logf;
use crate::settings::*;
use crate::util::{lock, panic_message};
use crate::win::platform::wide;
use crate::{is_portable, VERSION};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontIndirectW, CreateSolidBrush, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, EnumFontFamiliesExW, FillRect, GetDC, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, RedrawWindow, ReleaseDC,
    ScreenToClient, SelectObject, SetBkColor, SetBkMode, SetTextColor, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_EDITCONTROL, DT_END_ELLIPSIS,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FONT_CHARSET, FONT_QUALITY, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, LOGFONTW,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, SRCCOPY, TEXTMETRICW,
    TRANSPARENT,
};
use windows::Win32::Graphics::GdiPlus::{
    FillModeAlternate, GdipAddPathArc, GdipAddPathLine, GdipClosePathFigure, GdipCreateBitmapFromScan0, GdipCreateFromHDC, GdipCreatePath,
    GdipCreatePen1, GdipCreateSolidFill, GdipCreateTexture, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen,
    GdipDisposeImage, GdipDrawArc, GdipDrawEllipse, GdipDrawLines, GdipDrawPath, GdipFillEllipse, GdipFillPath, GdipScaleTextureTransform,
    GdipSetInterpolationMode, GdipSetPenEndCap, GdipSetPenLineJoin, GdipSetPenStartCap, GdipSetPixelOffsetMode, GdipSetSmoothingMode,
    GdipTranslateTextureTransform, GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics, GpImage, GpPath, GpPen,
    InterpolationModeHighQualityBicubic, LineCapRound, LineJoinRound, MatrixOrderAppend, PixelOffsetModeHalf, PointF,
    SmoothingModeAntiAlias, Status, UnitPixel, WrapModeTileFlipXY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetScrollInfo, SetWindowTheme, DRAWITEMSTRUCT, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DrawIconEx, GetClientRect,
    GetScrollInfo, GetSystemMetrics, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsChild, IsDialogMessageW, IsIconic,
    IsWindowVisible, KillTimer, LoadCursorW, MessageBoxW, PostMessageW, RegisterClassExW, SendMessageW, SetForegroundWindow, SetTimer,
    SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu, CW_USEDEFAULT, DI_NORMAL, HICON, HMENU, IDC_ARROW, IDNO, IDYES,
    MB_ICONQUESTION, MB_ICONWARNING, MB_YESNOCANCEL, MESSAGEBOX_STYLE, MF_CHECKED, MF_STRING, MINMAXINFO, MSG, SB_VERT, SCROLLINFO,
    SIF_ALL, SIF_PAGE, SIF_POS, SIF_RANGE, SM_CXICON, SM_CXSMICON, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
    SW_RESTORE, SW_SHOW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW,
};

// ---------- valeurs Win32 (winuser.h) ----------

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
const BS_OWNERDRAW: u32 = 0x000B;
const BS_NOTIFY: u32 = 0x4000;
const ES_PASSWORD: u32 = 0x0020;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_NUMBER: u32 = 0x2000;

const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_ACTIVATE: u32 = 0x0006;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_DRAWITEM: u32 = 0x002B;
const WM_SETFONT: u32 = 0x0030;
const WM_SETICON: u32 = 0x0080;
const WM_NCDESTROY: u32 = 0x0082;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const WM_VSCROLL: u32 = 0x0115;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_APP_DONE: u32 = 0x8000 + 20;
const DM_GETDEFID: u32 = 0x0400;
const DC_HASDEFID: isize = 0x534B;
const BM_SETSTYLE: u32 = 0x00F4;
const BN_CLICKED: u32 = 0;
const BN_DOUBLECLICKED: u32 = 5;
const BN_SETFOCUS: u32 = 6;
const EN_SETFOCUS: u32 = 0x0100;
const EN_KILLFOCUS: u32 = 0x0200;
const EN_CHANGE: u32 = 0x0300;
const EM_SETCUEBANNER: u32 = 0x1501;
const DLGC_WANTARROWS: isize = 0x0001;
const DLGC_UNDEFPUSHBUTTON: isize = 0x0020;
const DLGC_WANTCHARS: isize = 0x0080;
const DLGC_BUTTON: isize = 0x2000;
const ODS_SELECTED: u32 = 0x0001;
const ODS_DISABLED: u32 = 0x0004;
const ODS_FOCUS: u32 = 0x0010;
const ODS_NOFOCUSRECT: u32 = 0x0200;

const VK_TAB: usize = 0x09;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;

const ID_SAVE: usize = 1; // IDOK : touche Entrée
const ID_CLOSE: usize = 2; // IDCANCEL : touche Échap
const TIMER_STATUS: usize = 1;

const CLASS_MAIN: PCWSTR = w!("ImmichWallpaperSettings");
const CLASS_CONTENT: PCWSTR = w!("ImmichWallpaperSettingsContent");
const CLASS_GRID: PCWSTR = w!("ImmichWallpaperAlbumGrid");

// ---------- mise en page, en pixels à 100 % (96 ppp), mise à l'échelle de l'écran ----------

const DLG_W: i32 = 1000; // zone cliente à l'ouverture
const DLG_H: i32 = 700;
const MIN_W: i32 = 880;
const MIN_H: i32 = 560;
const SIDEBAR_W: i32 = 236;
const FOOTER_H: i32 = 68;
const PAD: i32 = 32; // marge du contenu
const FORM_W: i32 = 720; // largeur maximale des cartes de formulaire
const FIELD_H: i32 = 40;
const BTN_H: i32 = 38;
const RULES_W: i32 = 256;

const PAGE_SERVER: usize = 0;
const PAGE_ALBUMS: usize = 1;
const PAGE_DISPLAY: usize = 2;
const PAGE_SCHEDULE: usize = 3;
const PAGE_RULES: usize = 4;
const PAGE_CAPTION: usize = 5;
const PAGE_UPDATES: usize = 6;
const PAGES: [(&str, Icon); 7] = [
    ("Serveur", Icon::Server),
    ("Albums", Icon::Albums),
    ("Affichage", Icon::Display),
    ("Planification", Icon::Clock),
    ("Saisons et heures", Icon::Calendar),
    ("Légende", Icon::Pin),
    ("Mises à jour", Icon::Update),
];

const GRID_ALBUMS: usize = 0;
const GRID_RULE: usize = 1;

// ---------- textes ----------

const KEY_HINT: &str = "Immich → Paramètres du compte → Clés API. Permissions : album.read, asset.read, asset.view, \
                        asset.download (et asset.update pour « Ajouter aux favoris »).";
const HTTP_WARN: &str = "Connexion non chiffrée vers un serveur hors réseau local : le token et les photos circulent en clair. \
                         Préfère https://.";
const PIN_HINT: &str = "Le certificat présenté lors du test est mémorisé (empreinte SHA-256) : seul celui-ci sera accepté \
                        ensuite. S'il est renouvelé, reteste la connexion puis enregistre.";
const OFFLINE_HINT: &str = "Si Immich est injoignable (déplacement, VPN coupé…), la rotation continue avec les dernières \
                            photos téléchargées. 0 = désactivé.";
const HOURS_HINT: &str = "Laisse vide pour changer toute la journée. Une plage comme 22:00 → 07:00 passe par minuit.";
const PAUSES_HINT: &str = "Ne concerne que les changements automatiques : « Image suivante » fonctionne toujours. Le \
                           changement en attente se fait dès que la condition disparaît.";
const PRELOAD_HINT: &str = "Le changement suivant est alors immédiat et ne dépend plus du réseau, en particulier au \
                            déverrouillage du PC.";
const RULES_HINT: &str = "Quand une règle s'applique, ses albums remplacent les albums habituels ; plusieurs règles en même \
                          temps : leurs albums sont réunis.";
const LINK_HINT: &str = "Le fond d'écran n'est pas cliquable sous Windows : cette icône, ou « Voir la photo dans Immich » \
                         dans le menu de l'icône près de l'horloge, ouvre la photo affichée.";
const LOCK_HINT: &str = "Dans Windows : Paramètres → Personnalisation → Écran de verrouillage → « Image » (et non « Windows \
                         à la une »).";
const UPDATE_HINT: &str = "Vérification au démarrage puis une fois par jour, depuis les releases GitHub du projet \
                           uniquement ; l'empreinte SHA-256 détecte un fichier corrompu (les releases ne sont pas signées).";
const UPDATE_PORTABLE_HINT: &str = "Version portable : une nouvelle version est seulement annoncée dans le menu de l'icône \
                                    (« Installer la version… ») ; rien n'est téléchargé ni remplacé sans ton accord. \
                                    Décoché, l'appli ne contacte plus GitHub d'elle-même.";

// ---------- thème (couleurs d'Immich) ----------

/// Couleurs en 0xRRGGBB.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Theme {
    dark: bool,
    bg: u32,
    card: u32,
    field: u32,
    border: u32,
    fg: u32,
    muted: u32,
    primary: u32,
    on_primary: u32,
    soft: u32, // fond de l'élément sélectionné (barre latérale, règles)
    secondary: u32,
    track: u32, // interrupteur éteint
    divider: u32,
    ok: u32,
    err: u32,
}

const LIGHT: Theme = Theme {
    dark: false,
    bg: 0xFFFFFF,
    card: 0xF6F6F4,
    field: 0xFFFFFF,
    border: 0xDCDCD7,
    fg: 0x18181B,
    muted: 0x6B6B73,
    primary: 0x4250AF,
    on_primary: 0xFFFFFF,
    soft: 0xE7E9F6,
    secondary: 0xEEEEF2,
    track: 0xC7C8CE,
    divider: 0xE4E4E0,
    ok: 0x15803D,
    err: 0xC0262D,
};

const DARK: Theme = Theme {
    dark: true,
    bg: 0x000000,
    card: 0x212121,
    field: 0x2A2A2A,
    border: 0x3A3A3A,
    fg: 0xE5E7EB,
    muted: 0x9CA3AF,
    primary: 0xADCBFA,
    on_primary: 0x000000,
    soft: 0x232A35,
    secondary: 0x262626,
    track: 0x4B4B52,
    divider: 0x2E2E2E,
    ok: 0x4ADE80,
    err: 0xF87171,
};

/// Thème imposé (tests) : 0 = celui de Windows, 1 = clair, 2 = sombre.
static THEME_OVERRIDE: AtomicU8 = AtomicU8::new(0);

/// Windows est en mode sombre pour les applications.
fn system_dark() -> bool {
    match THEME_OVERRIDE.load(Ordering::SeqCst) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_u32("AppsUseLightTheme"))
        .map(|v| v == 0)
        .unwrap_or(false)
}

fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF))
}

fn argb(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

/// Mélange de deux couleurs (t = part de b).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Tone {
    #[default]
    Fg,
    Muted,
    Ok,
    Err,
}

impl Theme {
    fn tone(&self, t: Tone) -> u32 {
        match t {
            Tone::Fg => self.fg,
            Tone::Muted => self.muted,
            Tone::Ok => self.ok,
            Tone::Err => self.err,
        }
    }
    fn hover(&self, base: u32) -> u32 {
        mix(base, self.fg, if self.dark { 0.10 } else { 0.05 })
    }
}

// ---------- dessin : GDI+ pour les formes, GDI pour le texte ----------

fn gdiplus() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let _ = GdiplusStartup(&mut token, &input, std::ptr::null_mut());
    });
}

#[derive(Clone, Copy, Debug)]
struct Rf {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn rf(r: RECT) -> Rf {
    Rf {
        x: r.left as f32,
        y: r.top as f32,
        w: (r.right - r.left) as f32,
        h: (r.bottom - r.top) as f32,
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w.max(0),
        bottom: y + h.max(0),
    }
}

fn inset(r: RECT, d: i32) -> RECT {
    RECT {
        left: r.left + d,
        top: r.top + d,
        right: r.right - d,
        bottom: r.bottom - d,
    }
}

/// Icônes au trait, dessinées sur une grille de 24 × 24.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Icon {
    Server,
    Albums,
    Display,
    Clock,
    Calendar,
    Pin,
    Update,
    Search,
    Check,
    Chevron,
    Plus,
    Refresh,
    Next,
    Trash,
}

struct Canvas(*mut GpGraphics);

impl Canvas {
    fn new(dc: HDC) -> Canvas {
        gdiplus();
        let mut g = std::ptr::null_mut();
        unsafe {
            GdipCreateFromHDC(dc, &mut g);
            GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
            GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
        }
        Canvas(g)
    }

    fn round_path(r: Rf, rad: f32) -> *mut GpPath {
        let mut p = std::ptr::null_mut();
        unsafe {
            GdipCreatePath(FillModeAlternate, &mut p);
            let d = (rad.min(r.w / 2.0).min(r.h / 2.0) * 2.0).max(0.01);
            GdipAddPathArc(p, r.x, r.y, d, d, 180.0, 90.0);
            GdipAddPathArc(p, r.x + r.w - d, r.y, d, d, 270.0, 90.0);
            GdipAddPathArc(p, r.x + r.w - d, r.y + r.h - d, d, d, 0.0, 90.0);
            GdipAddPathArc(p, r.x, r.y + r.h - d, d, d, 90.0, 90.0);
            GdipClosePathFigure(p);
        }
        p
    }

    fn fill_path(&self, p: *mut GpPath, color: u32) {
        unsafe {
            let mut b = std::ptr::null_mut();
            GdipCreateSolidFill(color, &mut b);
            GdipFillPath(self.0, b as *mut GpBrush, p);
            GdipDeleteBrush(b as *mut GpBrush);
        }
    }

    fn pen(color: u32, width: f32) -> *mut GpPen {
        let mut pen = std::ptr::null_mut();
        unsafe {
            GdipCreatePen1(color, width, UnitPixel, &mut pen);
            GdipSetPenStartCap(pen, LineCapRound);
            GdipSetPenEndCap(pen, LineCapRound);
            GdipSetPenLineJoin(pen, LineJoinRound);
        }
        pen
    }

    fn fill_round(&self, r: Rf, rad: f32, rgb: u32) {
        let p = Self::round_path(r, rad);
        self.fill_path(p, argb(rgb));
        unsafe {
            GdipDeletePath(p);
        }
    }

    fn stroke_round(&self, r: Rf, rad: f32, color: u32, width: f32) {
        let p = Self::round_path(r, rad);
        unsafe {
            let pen = Self::pen(color, width);
            GdipDrawPath(self.0, pen, p);
            GdipDeletePen(pen);
            GdipDeletePath(p);
        }
    }

    fn fill_circle(&self, cx: f32, cy: f32, rad: f32, color: u32) {
        unsafe {
            let mut b = std::ptr::null_mut();
            GdipCreateSolidFill(color, &mut b);
            GdipFillEllipse(self.0, b as *mut GpBrush, cx - rad, cy - rad, rad * 2.0, rad * 2.0);
            GdipDeleteBrush(b as *mut GpBrush);
        }
    }

    fn stroke_circle(&self, cx: f32, cy: f32, rad: f32, color: u32, width: f32) {
        unsafe {
            let pen = Self::pen(color, width);
            GdipDrawEllipse(self.0, pen, cx - rad, cy - rad, rad * 2.0, rad * 2.0);
            GdipDeletePen(pen);
        }
    }

    fn lines(&self, pts: &[(f32, f32)], color: u32, width: f32) {
        let pts: Vec<PointF> = pts.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
        unsafe {
            let pen = Self::pen(color, width);
            GdipDrawLines(self.0, pen, pts.as_ptr(), pts.len() as i32);
            GdipDeletePen(pen);
        }
    }

    fn arc(&self, (cx, cy, rad): (f32, f32, f32), start: f32, sweep: f32, color: u32, width: f32) {
        unsafe {
            let pen = Self::pen(color, width);
            GdipDrawArc(self.0, pen, cx - rad, cy - rad, rad * 2.0, rad * 2.0, start, sweep);
            GdipDeletePen(pen);
        }
    }

    fn fill_poly(&self, pts: &[(f32, f32)], color: u32) {
        let mut p = std::ptr::null_mut();
        unsafe {
            GdipCreatePath(FillModeAlternate, &mut p);
            for w in pts.windows(2) {
                GdipAddPathLine(p, w[0].0, w[0].1, w[1].0, w[1].1);
            }
            GdipClosePathFigure(p);
            self.fill_path(p, color);
            GdipDeletePath(p);
        }
    }

    /// Vignette dans un rectangle arrondi (bords antialiasés : remplissage par une texture).
    fn image_round(&self, c: &Cover, r: Rf, rad: f32) {
        let side = c.side as i32;
        unsafe {
            let mut bmp = std::ptr::null_mut();
            if GdipCreateBitmapFromScan0(
                side,
                side,
                side * 4,
                0x0026_200A, /* 32bppARGB */
                Some(c.bgra.as_ptr()),
                &mut bmp,
            ) != Status(0)
                || bmp.is_null()
            {
                return;
            }
            let mut tex = std::ptr::null_mut();
            if GdipCreateTexture(bmp as *mut GpImage, WrapModeTileFlipXY, &mut tex) == Status(0) && !tex.is_null() {
                GdipScaleTextureTransform(tex, r.w / side as f32, r.h / side as f32, MatrixOrderAppend);
                GdipTranslateTextureTransform(tex, r.x, r.y, MatrixOrderAppend);
                let p = Self::round_path(r, rad);
                GdipFillPath(self.0, tex as *mut GpBrush, p);
                GdipDeletePath(p);
                GdipDeleteBrush(tex as *mut GpBrush);
            }
            GdipDisposeImage(bmp as *mut GpImage);
        }
    }

    fn icon(&self, icon: Icon, x: f32, y: f32, size: f32, rgb: u32) {
        let s = size / 24.0;
        let p = |a: f32, b: f32| (x + a * s, y + b * s);
        let c = argb(rgb);
        let wd = (1.8 * s).max(1.2);
        match icon {
            Icon::Server => {
                for top in [3.5, 13.0] {
                    let (x0, y0) = p(3.5, top);
                    self.stroke_round(
                        Rf {
                            x: x0,
                            y: y0,
                            w: 17.0 * s,
                            h: 7.5 * s,
                        },
                        2.2 * s,
                        c,
                        wd,
                    );
                    let (dx, dy) = p(7.5, top + 3.75);
                    self.fill_circle(dx, dy, 1.2 * s, c);
                }
            }
            Icon::Albums => {
                let (x0, y0) = p(3.0, 4.5);
                self.stroke_round(
                    Rf {
                        x: x0,
                        y: y0,
                        w: 18.0 * s,
                        h: 15.0 * s,
                    },
                    3.0 * s,
                    c,
                    wd,
                );
                self.lines(&[p(4.0, 17.5), p(9.5, 12.0), p(13.5, 16.0), p(16.0, 13.5), p(20.0, 17.5)], c, wd);
                let (sx, sy) = p(15.5, 9.0);
                self.fill_circle(sx, sy, 1.7 * s, c);
            }
            Icon::Display => {
                let (x0, y0) = p(3.0, 4.0);
                self.stroke_round(
                    Rf {
                        x: x0,
                        y: y0,
                        w: 18.0 * s,
                        h: 12.0 * s,
                    },
                    2.2 * s,
                    c,
                    wd,
                );
                self.lines(&[p(12.0, 16.0), p(12.0, 20.0)], c, wd);
                self.lines(&[p(8.0, 20.0), p(16.0, 20.0)], c, wd);
            }
            Icon::Clock => {
                let (cx, cy) = p(12.0, 12.0);
                self.stroke_circle(cx, cy, 8.5 * s, c, wd);
                self.lines(&[p(12.0, 7.5), p(12.0, 12.0), p(15.0, 14.0)], c, wd);
            }
            Icon::Calendar => {
                let (x0, y0) = p(3.5, 5.0);
                self.stroke_round(
                    Rf {
                        x: x0,
                        y: y0,
                        w: 17.0 * s,
                        h: 15.5 * s,
                    },
                    2.5 * s,
                    c,
                    wd,
                );
                self.lines(&[p(3.5, 10.0), p(20.5, 10.0)], c, wd);
                self.lines(&[p(8.0, 3.0), p(8.0, 6.5)], c, wd);
                self.lines(&[p(16.0, 3.0), p(16.0, 6.5)], c, wd);
            }
            Icon::Pin => {
                let (cx, cy) = p(12.0, 10.0);
                let rad = 6.5 * s;
                let mut path = std::ptr::null_mut();
                unsafe {
                    GdipCreatePath(FillModeAlternate, &mut path);
                    GdipAddPathArc(path, cx - rad, cy - rad, rad * 2.0, rad * 2.0, 140.0, 260.0);
                    let (tx, ty) = p(12.0, 21.0);
                    let e = 400.0f32.to_radians();
                    GdipAddPathLine(path, cx + rad * e.cos(), cy + rad * e.sin(), tx, ty);
                    GdipClosePathFigure(path);
                    let pen = Self::pen(c, wd);
                    GdipDrawPath(self.0, pen, path);
                    GdipDeletePen(pen);
                    GdipDeletePath(path);
                }
                self.stroke_circle(cx, cy, 2.3 * s, c, wd);
            }
            Icon::Update => {
                self.lines(&[p(12.0, 4.0), p(12.0, 15.0)], c, wd);
                self.lines(&[p(7.5, 10.5), p(12.0, 15.0), p(16.5, 10.5)], c, wd);
                self.lines(&[p(5.0, 19.5), p(19.0, 19.5)], c, wd);
            }
            Icon::Search => {
                let (cx, cy) = p(10.5, 10.5);
                self.stroke_circle(cx, cy, 6.0 * s, c, wd);
                self.lines(&[p(15.0, 15.0), p(20.0, 20.0)], c, wd);
            }
            Icon::Check => self.lines(&[p(5.0, 12.5), p(10.0, 17.0), p(19.0, 7.5)], c, wd * 1.2),
            Icon::Chevron => self.lines(&[p(7.0, 9.5), p(12.0, 14.5), p(17.0, 9.5)], c, wd),
            Icon::Plus => {
                self.lines(&[p(12.0, 5.0), p(12.0, 19.0)], c, wd);
                self.lines(&[p(5.0, 12.0), p(19.0, 12.0)], c, wd);
            }
            Icon::Refresh => {
                let (cx, cy) = p(12.0, 12.0);
                self.arc((cx, cy, 7.0 * s), 30.0, 290.0, c, wd);
                self.lines(&[p(19.5, 4.5), p(19.0, 9.0), p(14.5, 8.5)], c, wd);
            }
            Icon::Next => {
                self.fill_poly(&[p(6.0, 5.5), p(15.5, 12.0), p(6.0, 18.5), p(6.0, 5.5)], c);
                self.lines(&[p(18.5, 5.5), p(18.5, 18.5)], c, wd * 1.2);
            }
            Icon::Trash => {
                self.lines(&[p(4.0, 7.0), p(20.0, 7.0)], c, wd);
                self.lines(&[p(9.0, 7.0), p(9.5, 4.0), p(14.5, 4.0), p(15.0, 7.0)], c, wd);
                self.lines(&[p(6.0, 7.0), p(7.0, 20.0), p(17.0, 20.0), p(18.0, 7.0)], c, wd);
            }
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.0);
        }
    }
}

/// Dessin hors écran, recopié d'un coup (pas de scintillement).
struct Buffer {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    target: HDC,
    r: RECT,
}

impl Buffer {
    fn new(target: HDC, r: RECT) -> Buffer {
        unsafe {
            let dc = CreateCompatibleDC(Some(target));
            let bmp = CreateCompatibleBitmap(target, (r.right - r.left).max(1), (r.bottom - r.top).max(1));
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            SetBkMode(dc, TRANSPARENT);
            Buffer { dc, bmp, old, target, r }
        }
    }

    fn fill(&self, r: RECT, rgb: u32) {
        unsafe {
            let b = CreateSolidBrush(colorref(rgb));
            FillRect(self.dc, &r, b);
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            let r = self.r;
            let _ = BitBlt(
                self.target,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                Some(self.dc),
                0,
                0,
                SRCCOPY,
            );
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

// ---------- polices ----------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Font {
    Body,
    Semi,
    Small,
    Title,
}

struct Fonts {
    body: HFONT,
    semi: HFONT,
    small: HFONT,
    title: HFONT,
    brand: HFONT,
    nav: HFONT,
    /// Hauteur d'une ligne de texte courant (zones de saisie).
    line: i32,
}

/// La police est installée.
fn font_exists(name: &str) -> bool {
    unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, lp: LPARAM) -> i32 {
        *(lp.0 as *mut bool) = true;
        0
    }
    let mut lf = LOGFONTW {
        lfCharSet: FONT_CHARSET(1),
        ..Default::default()
    };
    for (d, s) in lf.lfFaceName.iter_mut().zip(name.encode_utf16().take(31)) {
        *d = s;
    }
    let mut ok = false;
    unsafe {
        let dc = GetDC(None);
        EnumFontFamiliesExW(dc, &lf, Some(found), LPARAM(&mut ok as *mut bool as isize), 0);
        ReleaseDC(None, dc);
    }
    ok
}

/// Polices de Windows : Segoe UI Variable (Windows 11), sinon Segoe UI.
fn faces() -> &'static (String, String) {
    static FACES: OnceLock<(String, String)> = OnceLock::new();
    FACES.get_or_init(|| {
        if font_exists("Segoe UI Variable Text") {
            ("Segoe UI Variable Text".into(), "Segoe UI Variable Display".into())
        } else {
            ("Segoe UI".into(), "Segoe UI".into())
        }
    })
}

fn make_font(face: &str, tenths_pt: i32, weight: i32, dpi: u32) -> HFONT {
    let mut lf = LOGFONTW {
        lfHeight: -mul_div(tenths_pt, dpi, 720),
        lfWeight: weight,
        lfCharSet: FONT_CHARSET(1),
        lfQuality: FONT_QUALITY(5), // ClearType
        ..Default::default()
    };
    for (d, s) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
        *d = s;
    }
    unsafe { CreateFontIndirectW(&lf) }
}

impl Fonts {
    fn new(dpi: u32) -> Fonts {
        let (text, display) = faces();
        let body = make_font(text, 100, 400, dpi);
        let line = text_h(body, "Ag", 1000) + mul_div(2, dpi, 96);
        Fonts {
            body,
            semi: make_font(text, 100, 600, dpi),
            small: make_font(text, 90, 400, dpi),
            title: make_font(display, 190, 600, dpi),
            brand: make_font(display, 115, 600, dpi),
            nav: make_font(text, 105, 400, dpi),
            line,
        }
    }

    fn get(&self, f: Font) -> HFONT {
        match f {
            Font::Body => self.body,
            Font::Semi => self.semi,
            Font::Small => self.small,
            Font::Title => self.title,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in [self.body, self.semi, self.small, self.title, self.brand, self.nav] {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}

fn draw_text(dc: HDC, font: HFONT, s: &str, r: RECT, rgb: u32, flags: DRAW_TEXT_FORMAT) {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    if buf.is_empty() {
        return;
    }
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(font.0));
        SetTextColor(dc, colorref(rgb));
        SetBkMode(dc, TRANSPARENT);
        let mut r = r;
        DrawTextW(dc, &mut buf, &mut r, flags | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// Hauteur du texte coupé à la largeur donnée.
fn text_h(font: HFONT, s: &str, width: i32) -> i32 {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    if buf.is_empty() {
        return 0;
    }
    unsafe {
        let dc = GetDC(None);
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut r = RECT {
            left: 0,
            top: 0,
            right: width.max(1),
            bottom: 0,
        };
        DrawTextW(dc, &mut buf, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX | DT_EDITCONTROL);
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        r.bottom - r.top
    }
}

/// Largeur d'un texte sur une ligne.
fn text_w(font: HFONT, s: &str) -> i32 {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    if buf.is_empty() {
        return 0;
    }
    unsafe {
        let dc = GetDC(None);
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut r = RECT::default();
        DrawTextW(dc, &mut buf, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        r.right - r.left
    }
}

// ---------- état partagé ----------

thread_local! {
    /// Fenêtre ouverte (une seule à la fois), sur le thread de l'interface.
    static CURRENT: RefCell<Option<Rc<Win>>> = const { RefCell::new(None) };
}

/// Fenêtre ouverte, lisible depuis les autres threads (vérification, tests).
static OPEN_HWND: AtomicIsize = AtomicIsize::new(0);
/// Albums chargés dans la fenêtre au dernier test de connexion réussi (vérification).
static LOADED_ALBUMS: AtomicUsize = AtomicUsize::new(0);
/// Contrôle survolé par la souris.
static HOT: AtomicIsize = AtomicIsize::new(0);

/// Résultats des tâches de fond, par fenêtre (handle), récupérés sur le thread de l'interface.
enum Done {
    Albums { quiet: bool, res: Result<Vec<AlbumInfo>, String> },
    Cover,
    Next(Result<String, String>),
}
static DONE: Mutex<Vec<(isize, Done)>> = Mutex::new(Vec::new());

fn current() -> Option<Rc<Win>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// La fenêtre de réglages est ouverte (depuis n'importe quel thread).
pub fn is_open() -> bool {
    OPEN_HWND.load(Ordering::SeqCst) != 0
}

/// Handle de la fenêtre ouverte (0 sinon).
pub fn open_hwnd() -> isize {
    OPEN_HWND.load(Ordering::SeqCst)
}

/// Nombre d'albums affichés après le dernier test de connexion réussi.
pub fn loaded_albums() -> usize {
    LOADED_ALBUMS.load(Ordering::SeqCst)
}

/// Ouvre la fenêtre, ou la ramène au premier plan si elle l'est déjà. Thread de l'interface
/// uniquement (celui qui fait tourner la boucle de messages).
pub fn open(settings: Arc<Settings>) {
    if let Some(w) = current() {
        w.bring_to_front();
        return;
    }
    match Win::create(settings) {
        Ok(w) => w.show(),
        Err(e) => {
            logf!("fenêtre de réglages : {e}");
            crate::win::platform::error(&format!("Impossible d'ouvrir les réglages : {e}"));
        }
    }
}

/// À appeler dans la boucle de messages avant TranslateMessage : navigation au clavier de la
/// fenêtre (Tab, Entrée, Échap, Ctrl+Tab). true : le message a été traité.
pub fn pre_translate(msg: &MSG) -> bool {
    let Some(w) = current() else { return false };
    unsafe {
        if msg.message == WM_KEYDOWN
            && msg.wParam.0 == VK_TAB
            && GetKeyState(VK_CONTROL) < 0
            && (msg.hwnd == w.hwnd || IsChild(w.hwnd, msg.hwnd).as_bool())
        {
            let n = PAGES.len();
            let d = if GetKeyState(VK_SHIFT) < 0 { n - 1 } else { 1 };
            let page = w.st.borrow().page;
            w.show_page((page + d) % n);
            let _ = SetFocus(Some(w.c.nav[(page + d) % n]));
            return true;
        }
        IsDialogMessageW(w.hwnd, msg).as_bool()
    }
}

// ---------- petites fonctions Win32 ----------

fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}

fn send_str(h: HWND, msg: u32, wp: usize, s: &str) -> isize {
    let t = wide(s);
    send(h, msg, wp, t.as_ptr() as isize)
}

fn set_text(h: HWND, s: &str) {
    let t = wide(s);
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR(t.as_ptr()));
    }
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(buf.len())])
    }
}

fn move_win(h: HWND, x: i32, y: i32, w: i32, ht: i32) {
    unsafe {
        let _ = SetWindowPos(h, None, x, y, w.max(0), ht.max(0), SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn move_rect(h: HWND, r: RECT) {
    move_win(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
}

fn show_win(h: HWND, on: bool) {
    unsafe {
        let _ = ShowWindow(h, if on { SW_SHOW } else { SW_HIDE });
    }
}

fn enable(h: HWND, on: bool) {
    unsafe {
        let _ = EnableWindow(h, on);
    }
    invalidate(h);
}

fn client_rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    r
}

fn invalidate(h: HWND) {
    if !h.is_invalid() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

fn mul_div(v: i32, num: u32, den: u32) -> i32 {
    ((v as i64 * num as i64 + (den as i64) / 2) / den as i64) as i32
}

fn hwnd_of(v: isize) -> HWND {
    HWND(v as *mut _)
}

/// Résultat d'une tâche de fond, remis à la fenêtre `hwnd` sur le thread de l'interface.
fn finish(hwnd: isize, d: Done) {
    lock(&DONE).push((hwnd, d));
    unsafe {
        let _ = PostMessageW(Some(hwnd_of(hwnd)), WM_APP_DONE, WPARAM(0), LPARAM(0));
    }
}

// ---------- contrôles dessinés par l'appli ----------

/// Fond derrière un contrôle (pour ses coins arrondis).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Bg {
    Page,
    Card,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Nav(usize),
    Primary,
    Secondary,
    Danger,
    /// Ligne de réglage entière : titre, description et interrupteur.
    Toggle,
    /// Liste déroulante (menu de Windows à l'ouverture).
    Select(&'static [Choice]),
    Rule(usize),
}

impl Kind {
    /// Bouton « poussoir » : la touche Entrée l'active quand il a le focus.
    fn push_like(&self) -> bool {
        !matches!(self, Kind::Toggle | Kind::Select(_))
    }
}

#[derive(Clone)]
struct Owner {
    kind: Kind,
    text: String,
    desc: String,
    icon: Option<Icon>,
    on: bool,
    sel: usize,
    bg: Bg,
}

/// Survol : le contrôle se redessine à l'entrée et à la sortie de la souris.
unsafe extern "system" fn owner_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        BM_SETSTYLE => return LRESULT(0), // IsDialogMessage ne doit pas lui retirer son dessin personnalisé
        WM_GETDLGCODE => {
            let push = current()
                .and_then(|w| w.owners.borrow().get(&(h.0 as isize)).map(|o| o.kind.push_like()))
                .unwrap_or(false);
            if push {
                return LRESULT(DLGC_BUTTON | DLGC_UNDEFPUSHBUTTON);
            }
        }
        WM_MOUSEMOVE => {
            let old = HOT.swap(h.0 as isize, Ordering::SeqCst);
            if old != h.0 as isize {
                invalidate(hwnd_of(old));
                invalidate(h);
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: h,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut t);
            }
        }
        WM_MOUSELEAVE if HOT.compare_exchange(h.0 as isize, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() => invalidate(h),
        _ => {}
    }
    DefSubclassProc(h, msg, wp, lp)
}

/// Crée un contrôle enfant (police courante, identifiant).
fn create(hinst: HINSTANCE, parent: HWND, class: PCWSTR, text: &str, style: u32, ex: u32, id: usize) -> HWND {
    let t = wide(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(ex),
            class,
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(style | WS_CHILD | WS_VISIBLE),
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut _)),
            Some(hinst),
            None,
        )
    }
    .unwrap_or_else(|e| panic!("création d'un contrôle : {e}"))
}

fn create_owner(hinst: HINSTANCE, parent: HWND, id: usize, text: &str) -> HWND {
    let h = create(hinst, parent, w!("BUTTON"), text, BS_OWNERDRAW | BS_NOTIFY | WS_TABSTOP, 0, id);
    unsafe {
        let _ = SetWindowSubclass(h, Some(owner_proc), 1, 0);
    }
    h
}

// ---------- contrôles ----------

/// Handles des contrôles permanents.
#[derive(Default, Clone, Copy)]
struct Ctl {
    nav: [HWND; 7],
    content: HWND,
    b_next: HWND,
    b_close: HWND,
    b_save: HWND,
    // Serveur
    e_url: HWND,
    e_key: HWND,
    t_insecure: HWND,
    b_test: HWND,
    // Albums
    e_filter: HWND,
    b_reload: HWND,
    grid_albums: HWND,
    // Affichage
    e_interval: HWND,
    s_style: HWND,
    s_size: HWND,
    s_monitor: HWND,
    e_offline: HWND,
    t_split: HWND,
    t_landscape: HWND,
    t_start: HWND,
    t_unlock: HWND,
    b_show_excluded: HWND,
    // Planification
    e_from: HWND,
    e_to: HWND,
    t_battery: HWND,
    t_metered: HWND,
    t_fullscreen: HWND,
    t_preload: HWND,
    // Saisons et heures
    b_add_rule: HWND,
    b_del_rule: HWND,
    s_season: HWND,
    e_rfrom: HWND,
    e_rto: HWND,
    grid_rule: HWND,
    // Légende
    t_place: HWND,
    t_date: HWND,
    s_caption: HWND,
    t_link: HWND,
    t_lock: HWND,
    // Mises à jour
    t_update: HWND,
}

/// Création des contrôles : identifiants, ordre de tabulation, pages.
struct Builder {
    hinst: HINSTANCE,
    font: HFONT,
    next_id: usize,
    owners: HashMap<isize, Owner>,
    edits: Vec<HWND>,
    pages: [Vec<HWND>; 7],
    page: usize,
}

impl Builder {
    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn owner_id(&mut self, parent: HWND, id: usize, kind: Kind, text: &str, desc: &str, icon: Option<Icon>) -> HWND {
        let h = create_owner(self.hinst, parent, id, text);
        let bg = if matches!(kind, Kind::Toggle | Kind::Select(_)) {
            Bg::Card
        } else {
            Bg::Page
        };
        self.owners.insert(
            h.0 as isize,
            Owner {
                kind,
                text: text.to_string(),
                desc: desc.to_string(),
                icon,
                on: false,
                sel: 0,
                bg,
            },
        );
        h
    }

    fn owner(&mut self, parent: HWND, kind: Kind, text: &str, desc: &str, icon: Option<Icon>) -> HWND {
        let id = self.id();
        let h = self.owner_id(parent, id, kind, text, desc, icon);
        self.pages[self.page].push(h);
        h
    }

    fn toggle(&mut self, p: HWND, title: &str, desc: &str) -> HWND {
        self.owner(p, Kind::Toggle, title, desc, None)
    }

    fn select(&mut self, p: HWND, list: &'static [Choice]) -> HWND {
        self.owner(p, Kind::Select(list), "", "", None)
    }

    fn edit(&mut self, p: HWND, style: u32, cue: &str) -> HWND {
        let id = self.id();
        let h = create(self.hinst, p, w!("EDIT"), "", ES_AUTOHSCROLL | WS_TABSTOP | style, 0, id);
        send(h, WM_SETFONT, self.font.0 as usize, 0);
        if !cue.is_empty() {
            send_str(h, EM_SETCUEBANNER, 1, cue);
        }
        self.edits.push(h);
        self.pages[self.page].push(h);
        h
    }

    fn grid(&mut self, p: HWND) -> HWND {
        let id = self.id();
        let h = create(self.hinst, p, CLASS_GRID, "", WS_TABSTOP | WS_VSCROLL, 0, id);
        self.pages[self.page].push(h);
        h
    }
}

fn build(b: &mut Builder, main: HWND) -> Ctl {
    let mut c = Ctl::default();
    for (i, (name, icon)) in PAGES.iter().enumerate() {
        let id = b.id();
        c.nav[i] = b.owner_id(main, id, Kind::Nav(i), name, "", Some(*icon));
    }
    let id = b.id();
    c.content = create(
        b.hinst,
        main,
        CLASS_CONTENT,
        "",
        WS_CLIPCHILDREN | WS_VSCROLL,
        WS_EX_CONTROLPARENT,
        id,
    );
    let p = c.content;

    b.page = PAGE_SERVER;
    c.e_url = b.edit(p, 0, "https://immich.exemple.fr");
    c.e_key = b.edit(p, ES_PASSWORD, "");
    c.t_insecure = b.toggle(p, "Accepter un certificat auto-signé", PIN_HINT);
    c.b_test = b.owner(p, Kind::Primary, "Tester la connexion", "", Some(Icon::Refresh));

    b.page = PAGE_ALBUMS;
    c.e_filter = b.edit(p, 0, "Rechercher un album");
    c.b_reload = b.owner(p, Kind::Secondary, "Recharger", "", Some(Icon::Refresh));
    c.grid_albums = b.grid(p);

    b.page = PAGE_DISPLAY;
    c.e_interval = b.edit(p, ES_NUMBER, "");
    c.s_style = b.select(p, STYLES);
    c.s_size = b.select(p, SIZES);
    c.s_monitor = b.select(p, MONITORS);
    c.e_offline = b.edit(p, ES_NUMBER, "");
    c.t_split = b.toggle(
        p,
        "Écran très large (32:9)",
        "Deux photos côte à côte au lieu d'une seule très rognée.",
    );
    c.t_landscape = b.toggle(p, "Uniquement les photos en paysage", "");
    c.t_start = b.toggle(p, "Changer d'image au démarrage", "");
    c.t_unlock = b.toggle(p, "Changer d'image à chaque déverrouillage du PC", "");
    c.b_show_excluded = b.owner(p, Kind::Secondary, "Les réafficher", "", None);

    b.page = PAGE_SCHEDULE;
    c.e_from = b.edit(p, 0, "--:--");
    c.e_to = b.edit(p, 0, "--:--");
    c.t_battery = b.toggle(p, "Pause sur batterie", "");
    c.t_metered = b.toggle(p, "Pause sur une connexion limitée", "Partage de connexion, forfait de données.");
    c.t_fullscreen = b.toggle(p, "Pause pendant une application en plein écran", "Jeu, vidéo, présentation.");
    c.t_preload = b.toggle(p, "Télécharger la photo suivante à l'avance", PRELOAD_HINT);

    b.page = PAGE_RULES;
    c.b_add_rule = b.owner(p, Kind::Secondary, "Ajouter une règle", "", Some(Icon::Plus));
    c.b_del_rule = b.owner(p, Kind::Danger, "Supprimer", "", Some(Icon::Trash));
    c.s_season = b.select(p, SEASONS);
    c.e_rfrom = b.edit(p, 0, "--:--");
    c.e_rto = b.edit(p, 0, "--:--");
    c.grid_rule = b.grid(p);

    b.page = PAGE_CAPTION;
    c.t_place = b.toggle(p, "Afficher le lieu", "Ville, région, pays — photos géolocalisées.");
    c.t_date = b.toggle(p, "Afficher le mois et l'année", "");
    c.s_caption = b.select(p, CAPTIONS);
    c.t_link = b.toggle(p, "Icône « Voir cette photo dans Immich » sur le bureau", LINK_HINT);
    c.t_lock = b.toggle(p, "Appliquer aussi à l'écran de verrouillage", LOCK_HINT);

    b.page = PAGE_UPDATES;
    c.t_update = b.toggle(p, "Installer automatiquement les nouvelles versions", UPDATE_HINT);

    // Barre du bas (hors des pages).
    let id = b.id();
    c.b_next = b.owner_id(main, id, Kind::Secondary, "Image suivante", "", Some(Icon::Next));
    c.b_close = b.owner_id(main, ID_CLOSE, Kind::Secondary, "Fermer", "", None);
    c.b_save = b.owner_id(main, ID_SAVE, Kind::Primary, "Enregistrer", "", None);
    c
}

// ---------- fenêtre ----------

/// Élément dessiné sur la page (coordonnées de la zone de contenu, défilement compris).
enum Draw {
    Card(RECT),
    Field(RECT, HWND),
    Divider(RECT),
    Text(RECT, String, Font, Tone, DRAW_TEXT_FORMAT),
    Icon(RECT, Icon, Tone),
}

#[derive(Default)]
struct Grid {
    rows: Vec<AlbumRow>,
    scroll: i32,
    total: i32,
    hot: Option<usize>,
    focus: usize,
}

struct Geom {
    cols: usize,
    cell_w: i32,
    cell_h: i32,
    gap: i32,
}

#[derive(Default)]
struct State {
    albums: Vec<AlbumInfo>, // albums chargés depuis Immich (triés)
    selected: Vec<String>,  // albums habituels cochés, dans l'ordre
    rules: Vec<AlbumRule>,
    cur: Option<usize>, // règle sélectionnée
    rule_btns: Vec<HWND>,
    has_key: bool,
    cert_fp: String,
    http_warn: bool,
    excluded: usize,
    testing: bool,
    changing: bool,
    clean: String, // formulaire tel qu'enregistré (détection des modifications)
    last_status: String,
    page: usize,
    focus: isize, // contrôle à qui rendre le clavier quand la fenêtre est réactivée
    scroll: i32,
    total: i32,
    test_msg: (String, Tone),
    msg: (String, Tone),
    cover_server: String, // serveur des vignettes affichées
}

/// Réponse d'une boîte de message (remplaçable par les tests).
type Alert = Box<dyn FnMut(&str, MESSAGEBOX_STYLE) -> i32>;

struct Win {
    settings: Arc<Settings>,
    hwnd: HWND,
    hinst: HINSTANCE,
    c: Ctl,
    edits: Vec<HWND>,
    pages: [Vec<HWND>; 7],
    next_id: Cell<usize>,
    fonts: RefCell<Fonts>,
    dpi: Cell<u32>,
    theme: Cell<Theme>,
    field_brush: Cell<HBRUSH>,
    icons: Cell<(HICON, HICON, HICON)>, // grande, petite, marque
    ready: Cell<bool>,
    in_layout: Cell<bool>,
    /// Remplissage en cours : les notifications des zones de saisie sont ignorées.
    filling: Cell<bool>,
    portable: bool,
    st: RefCell<State>,
    owners: RefCell<HashMap<isize, Owner>>,
    grids: [RefCell<Grid>; 2],
    draws: RefCell<Vec<Draw>>,
    alert: RefCell<Option<Alert>>,
}

fn register_classes(hinst: HINSTANCE) -> Result<(), String> {
    static ONCE: Once = Once::new();
    static ERR: Mutex<Option<String>> = Mutex::new(None);
    ONCE.call_once(|| unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for class in [CLASS_MAIN, CLASS_CONTENT, CLASS_GRID] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: windows::Win32::UI::WindowsAndMessaging::WNDCLASS_STYLES(0x0008), // CS_DBLCLKS
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                hCursor: cursor,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                *lock(&ERR) = Some(format!("RegisterClassEx : {}", windows::core::Error::from_thread()));
            }
        }
    });
    match lock(&ERR).clone() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(w) = current() {
        if w.hwnd == hwnd || w.c.content == hwnd || w.grid_of(hwnd).is_some() {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.handle(hwnd, msg, wp, lp))) {
                Ok(Some(r)) => return r,
                Ok(None) => {}
                Err(p) => logf!("panic (fenêtre de réglages) : {}", panic_message(&*p)),
            }
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Place les éléments d'une page de haut en bas (cartes, lignes de réglage).
struct Flow<'a> {
    w: &'a Win,
    x: i32,
    y: i32,
    cw: i32,
    draws: Vec<Draw>,
    card: Option<(usize, i32)>,
    rows: usize,
}

impl Flow<'_> {
    fn p(&self, v: i32) -> i32 {
        self.w.px(v)
    }

    fn font(&self, f: Font) -> HFONT {
        self.w.fonts.borrow().get(f)
    }

    fn text(&mut self, r: RECT, s: &str, f: Font, t: Tone) {
        self.draws.push(Draw::Text(r, s.to_string(), f, t, DT_WORDBREAK | DT_EDITCONTROL));
    }

    fn header(&mut self, title: &str, sub: &str) {
        let th = text_h(self.font(Font::Title), title, self.cw);
        self.text(rect(self.x, self.y, self.cw, th), title, Font::Title, Tone::Fg);
        self.y += th + self.p(4);
        if !sub.is_empty() {
            let sh = text_h(self.font(Font::Body), sub, self.cw);
            self.text(rect(self.x, self.y, self.cw, sh), sub, Font::Body, Tone::Muted);
            self.y += sh;
        }
        self.y += self.p(22);
    }

    fn note(&mut self, s: &str, t: Tone) {
        let h = text_h(self.font(Font::Small), s, self.cw - self.p(8));
        self.text(rect(self.x + self.p(4), self.y, self.cw - self.p(8), h), s, Font::Small, t);
        self.y += h + self.p(16);
    }

    fn begin_card(&mut self) {
        self.card = Some((self.draws.len(), self.y));
        self.rows = 0;
        self.y += self.p(6);
    }

    fn end_card(&mut self) {
        if let Some((i, top)) = self.card.take() {
            self.y += self.p(6);
            self.draws.insert(i, Draw::Card(rect(self.x, top, self.cw, self.y - top)));
            self.y += self.p(16);
        }
    }

    fn sep(&mut self) {
        if self.rows > 0 {
            let d = rect(self.x + self.p(20), self.y, self.cw - self.p(40), self.p(1).max(1));
            self.draws.push(Draw::Divider(d));
        }
        self.rows += 1;
    }

    fn inner(&self) -> (i32, i32) {
        (self.x + self.p(20), self.cw - self.p(40))
    }

    /// Titre et description d'une ligne ; renvoie leur hauteur.
    fn block_h(&self, title: &str, desc: &str, width: i32) -> i32 {
        let mut h = text_h(self.font(Font::Semi), title, width);
        if !desc.is_empty() {
            h += self.p(3) + text_h(self.font(Font::Small), desc, width);
        }
        h
    }

    fn block(&mut self, x: i32, y: i32, width: i32, title: &str, desc: &str) {
        let th = text_h(self.font(Font::Semi), title, width);
        self.text(rect(x, y, width, th), title, Font::Semi, Tone::Fg);
        if !desc.is_empty() {
            let dh = text_h(self.font(Font::Small), desc, width);
            self.text(rect(x, y + th + self.p(3), width, dh), desc, Font::Small, Tone::Muted);
        }
    }

    /// Zone de saisie sous son libellé, sur toute la largeur de la carte.
    fn field(&mut self, label: &str, edit: HWND, hint: &str, warn: &str) {
        self.sep();
        let (ix, iw) = self.inner();
        self.y += self.p(16);
        let lh = text_h(self.font(Font::Semi), label, iw);
        self.text(rect(ix, self.y, iw, lh), label, Font::Semi, Tone::Fg);
        self.y += lh + self.p(8);
        let r = rect(ix, self.y, iw, self.p(FIELD_H));
        self.place_field(r, edit, 0);
        self.y += self.p(FIELD_H);
        for (s, t) in [(warn, Tone::Err), (hint, Tone::Muted)] {
            if !s.is_empty() {
                self.y += self.p(8);
                let h = text_h(self.font(Font::Small), s, iw);
                self.text(rect(ix, self.y, iw, h), s, Font::Small, t);
                self.y += h;
            }
        }
        self.y += self.p(16);
    }

    /// Cadre arrondi d'une zone de saisie ; `left` : place réservée à une icône.
    fn place_field(&mut self, r: RECT, edit: HWND, left: i32) {
        self.draws.push(Draw::Field(r, edit));
        let line = self.w.fonts.borrow().line;
        let pad = self.p(12);
        move_win(
            edit,
            r.left + pad + left,
            r.top + (r.bottom - r.top - line) / 2,
            r.right - r.left - 2 * pad - left,
            line,
        );
    }

    /// Ligne « interrupteur » : le bouton couvre toute la ligne (titre, description, interrupteur).
    fn toggle(&mut self, t: HWND) {
        self.sep();
        let (title, desc) = self.w.owner_texts(t);
        let width = self.cw - self.p(16) - 2 * self.p(14) - self.p(44) - self.p(16);
        let h = (self.block_h(&title, &desc, width) + self.p(28)).max(self.p(56));
        move_win(t, self.x + self.p(8), self.y + self.p(4), self.cw - self.p(16), h - self.p(8));
        self.w.set_bg(t, Bg::Card);
        self.y += h;
    }

    /// Ligne « titre à gauche, contrôles à droite » ; renvoie (haut, hauteur, x des contrôles).
    fn row(&mut self, title: &str, desc: &str, right_w: i32) -> (i32, i32, i32) {
        self.sep();
        let (ix, iw) = self.inner();
        let tw = iw - right_w - self.p(20);
        let th = self.block_h(title, desc, tw);
        let h = th.max(self.p(FIELD_H)) + self.p(28);
        let top = self.y;
        self.block(ix, top + (h - th) / 2, tw, title, desc);
        self.y += h;
        (top, h, ix + iw - right_w)
    }

    fn select_row(&mut self, title: &str, desc: &str, s: HWND) {
        let wd = self.p(260);
        let (top, h, x) = self.row(title, desc, wd);
        move_win(s, x, top + (h - self.p(FIELD_H)) / 2, wd, self.p(FIELD_H));
        self.w.set_bg(s, Bg::Card);
    }

    fn number_row(&mut self, title: &str, desc: &str, e: HWND, suffix: &str) {
        let fw = self.p(96);
        let sw = if suffix.is_empty() {
            0
        } else {
            text_w(self.font(Font::Body), suffix) + self.p(10)
        };
        let (top, h, x) = self.row(title, desc, fw + sw);
        let r = rect(x, top + (h - self.p(FIELD_H)) / 2, fw, self.p(FIELD_H));
        self.place_field(r, e, 0);
        if sw > 0 {
            let sr = rect(r.right + self.p(10), r.top, sw, self.p(FIELD_H));
            self.draws.push(Draw::Text(
                sr,
                suffix.to_string(),
                Font::Body,
                Tone::Muted,
                DT_SINGLELINE | DT_VCENTER,
            ));
        }
    }

    /// Deux heures « de … à … ».
    fn hours(&mut self, top: i32, h: i32, x: i32, from: HWND, to: HWND) {
        let fw = self.p(88);
        let y = top + (h - self.p(FIELD_H)) / 2;
        self.place_field(rect(x, y, fw, self.p(FIELD_H)), from, 0);
        let ar = rect(x + fw, y, self.p(32), self.p(FIELD_H));
        let center = DT_SINGLELINE | DT_VCENTER | windows::Win32::Graphics::Gdi::DT_CENTER;
        self.draws.push(Draw::Text(ar, "à".into(), Font::Body, Tone::Muted, center));
        self.place_field(rect(x + fw + self.p(32), y, fw, self.p(FIELD_H)), to, 0);
    }
}

impl Win {
    fn create(settings: Arc<Settings>) -> Result<Rc<Win>, String> {
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_STANDARD_CLASSES,
            };
            let _ = InitCommonControlsEx(&icc);
        }
        gdiplus();
        let hinst: HINSTANCE = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?.into();
        register_classes(hinst)?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_CONTROLPARENT),
                CLASS_MAIN,
                w!("Immich Wallpaper — Paramètres"),
                WINDOW_STYLE(WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                None,
                None,
                Some(hinst),
                None,
            )
        }
        .map_err(|e| format!("CreateWindowEx : {e}"))?;
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let fonts = Fonts::new(dpi);
        let mut b = Builder {
            hinst,
            font: fonts.body,
            next_id: 100,
            owners: HashMap::new(),
            edits: vec![],
            pages: Default::default(),
            page: 0,
        };
        let c = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut b, hwnd))) {
            Ok(c) => c,
            Err(p) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return Err(panic_message(&*p));
            }
        };
        let theme = if system_dark() { DARK } else { LIGHT };
        let w = Rc::new(Win {
            settings,
            hwnd,
            hinst,
            c,
            edits: b.edits,
            pages: b.pages,
            next_id: Cell::new(b.next_id + 1000),
            fonts: RefCell::new(fonts),
            dpi: Cell::new(dpi),
            theme: Cell::new(theme),
            field_brush: Cell::new(HBRUSH::default()),
            icons: Cell::new(Default::default()),
            ready: Cell::new(false),
            in_layout: Cell::new(false),
            filling: Cell::new(false),
            portable: is_portable(),
            st: RefCell::new(State::default()),
            owners: RefCell::new(b.owners),
            grids: Default::default(),
            draws: RefCell::new(vec![]),
            alert: RefCell::new(None),
        });
        CURRENT.with(|cur| *cur.borrow_mut() = Some(w.clone()));
        OPEN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        w.apply_theme(theme);
        w.set_icons();
        w.st.borrow_mut().focus = w.c.e_url.0 as isize;
        w.load();
        w.show_page(PAGE_SERVER);
        w.ready.set(true);
        w.place_default();
        w.layout();
        Ok(w)
    }

    fn px(&self, v: i32) -> i32 {
        mul_div(v, self.dpi.get(), 96)
    }

    fn t(&self) -> Theme {
        self.theme.get()
    }

    fn grid_of(&self, h: HWND) -> Option<usize> {
        if h == self.c.grid_albums {
            Some(GRID_ALBUMS)
        } else if h == self.c.grid_rule {
            Some(GRID_RULE)
        } else {
            None
        }
    }

    fn grid_hwnd(&self, i: usize) -> HWND {
        if i == GRID_ALBUMS {
            self.c.grid_albums
        } else {
            self.c.grid_rule
        }
    }

    fn apply_theme(&self, t: Theme) {
        self.theme.set(t);
        unsafe {
            let old = self.field_brush.replace(CreateSolidBrush(colorref(t.field)));
            if !old.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
            let on: i32 = t.dark as i32;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWINDOWATTRIBUTE(20), &on as *const i32 as *const _, 4);
            let scroll_theme = if t.dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
            for h in [self.c.content, self.c.grid_albums, self.c.grid_rule] {
                let _ = SetWindowTheme(h, scroll_theme, PCWSTR::null());
            }
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME,
            );
        }
    }

    fn set_icons(&self) {
        unsafe {
            let big = crate::win::app::load_icon(GetSystemMetrics(SM_CXICON)).unwrap_or_default();
            let small = crate::win::app::load_icon(GetSystemMetrics(SM_CXSMICON)).unwrap_or_default();
            let brand = crate::win::app::load_icon(self.px(32)).unwrap_or_default();
            send(self.hwnd, WM_SETICON, 1 /* ICON_BIG */, big.0 as isize);
            send(self.hwnd, WM_SETICON, 0 /* ICON_SMALL */, small.0 as isize);
            for i in [self.icons.replace((big, small, brand))] {
                for h in [i.0, i.1, i.2] {
                    if !h.is_invalid() {
                        let _ = DestroyIcon(h);
                    }
                }
            }
        }
    }

    /// Taille d'ouverture, centrée sur l'écran de la fenêtre.
    fn place_default(&self) {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: self.px(DLG_W),
            bottom: self.px(DLG_H),
        };
        unsafe {
            let _ = AdjustWindowRectExForDpi(
                &mut r,
                WINDOW_STYLE(WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN),
                false,
                WINDOW_EX_STYLE(WS_EX_CONTROLPARENT),
                self.dpi.get(),
            );
            let (mut ww, mut wh) = (r.right - r.left, r.bottom - r.top);
            let (mut x, mut y) = (0, 0);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
                let wa = mi.rcWork;
                ww = ww.min(wa.right - wa.left);
                wh = wh.min(wa.bottom - wa.top);
                x = wa.left + (wa.right - wa.left - ww) / 2;
                y = wa.top + (wa.bottom - wa.top - wh) / 2;
            }
            let _ = SetWindowPos(self.hwnd, None, x, y, ww, wh, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            SetTimer(Some(self.hwnd), TIMER_STATUS, 2000, None);
        }
        self.bring_to_front();
        self.refresh_status();
    }

    /// Affiche la fenêtre au premier plan, qu'elle soit cachée (lancement « caché » de l'appli)
    /// ou réduite.
    fn bring_to_front(&self) {
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    fn message_box(&self, text: &str, flags: MESSAGEBOX_STYLE) -> i32 {
        if let Some(f) = self.alert.borrow_mut().as_mut() {
            return f(text, flags);
        }
        unsafe { MessageBoxW(Some(self.hwnd), &HSTRING::from(text), w!("Immich Wallpaper"), flags).0 }
    }

    /// Libère les ressources une fois la fenêtre et ses contrôles détruits.
    fn release(&self) {
        unsafe {
            let (a, b, c) = self.icons.get();
            for i in [a, b, c] {
                if !i.is_invalid() {
                    let _ = DestroyIcon(i);
                }
            }
            let _ = DeleteObject(HGDIOBJ(self.field_brush.get().0));
        }
        lock(&DONE).retain(|(h, _)| *h != self.hwnd.0 as isize);
        OPEN_HWND.store(0, Ordering::SeqCst);
        CURRENT.with(|c| *c.borrow_mut() = None);
    }

    // ---------- contrôles dessinés ----------

    fn owner_texts(&self, h: HWND) -> (String, String) {
        self.owners
            .borrow()
            .get(&(h.0 as isize))
            .map(|o| (o.text.clone(), o.desc.clone()))
            .unwrap_or_default()
    }

    fn set_owner(&self, h: HWND, f: impl FnOnce(&mut Owner)) {
        if let Some(o) = self.owners.borrow_mut().get_mut(&(h.0 as isize)) {
            f(o);
        }
        invalidate(h);
    }

    fn set_bg(&self, h: HWND, bg: Bg) {
        if let Some(o) = self.owners.borrow_mut().get_mut(&(h.0 as isize)) {
            o.bg = bg;
        }
    }

    fn on(&self, h: HWND) -> bool {
        self.owners.borrow().get(&(h.0 as isize)).map(|o| o.on).unwrap_or(false)
    }

    fn set_on(&self, h: HWND, v: bool) {
        self.set_owner(h, |o| o.on = v);
    }

    fn sel(&self, h: HWND) -> usize {
        self.owners.borrow().get(&(h.0 as isize)).map(|o| o.sel).unwrap_or(0)
    }

    fn set_sel(&self, h: HWND, i: usize) {
        self.set_owner(h, |o| o.sel = i);
    }

    fn sel_value(&self, h: HWND, list: &[Choice]) -> String {
        choice_value(list, self.sel(h) as i32)
    }

    /// Dessine un bouton « owner-draw ».
    fn draw_item(&self, d: &DRAWITEMSTRUCT) {
        let Some(o) = self.owners.borrow().get(&(d.hwndItem.0 as isize)).cloned() else {
            return;
        };
        let t = self.t();
        let p = |v| self.px(v);
        let state = d.itemState.0;
        let hot = HOT.load(Ordering::SeqCst) == d.hwndItem.0 as isize;
        let pressed = state & ODS_SELECTED != 0;
        let disabled = state & ODS_DISABLED != 0;
        let focus = state & ODS_FOCUS != 0 && state & ODS_NOFOCUSRECT == 0;
        let r = d.rcItem;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let full = rect(0, 0, w, h);
        let bg = match o.kind {
            Kind::Nav(_) => t.bg,
            _ => match o.bg {
                Bg::Card => t.card,
                Bg::Page => t.bg,
            },
        };
        let buf = Buffer::new(d.hDC, r);
        buf.fill(full, bg);
        let fonts = self.fonts.borrow();
        let st_page = self.st.borrow().page;
        let cur = self.st.borrow().cur;
        let mut texts: Vec<(RECT, String, HFONT, u32, DRAW_TEXT_FORMAT)> = vec![];
        {
            let cv = Canvas::new(buf.dc);
            let radius = match o.kind {
                Kind::Toggle | Kind::Rule(_) => p(12) as f32,
                Kind::Select(_) => p(10) as f32,
                _ => h as f32 / 2.0,
            };
            match o.kind {
                Kind::Nav(i) => {
                    let sel = st_page == i;
                    let fill = if sel {
                        Some(t.soft)
                    } else if hot {
                        Some(t.hover(bg))
                    } else {
                        None
                    };
                    if let Some(f) = fill {
                        cv.fill_round(rf(full), radius, f);
                    }
                    let color = if sel { t.primary } else { t.fg };
                    if let Some(ic) = o.icon {
                        let s = p(20);
                        cv.icon(
                            ic,
                            p(16) as f32,
                            ((h - s) / 2) as f32,
                            s as f32,
                            if sel { t.primary } else { t.muted },
                        );
                    }
                    let font = if sel { fonts.semi } else { fonts.nav };
                    texts.push((
                        rect(p(48), 0, w - p(56), h),
                        o.text.clone(),
                        font,
                        color,
                        DT_SINGLELINE | DT_VCENTER,
                    ));
                }
                Kind::Primary | Kind::Secondary | Kind::Danger => {
                    let (mut fill, mut fg) = match o.kind {
                        Kind::Primary => (t.primary, t.on_primary),
                        Kind::Danger => (t.secondary, t.err),
                        _ => (t.secondary, t.fg),
                    };
                    if disabled {
                        fill = mix(fill, bg, 0.5);
                        fg = mix(fg, fill, 0.5);
                    } else if pressed {
                        fill = mix(fill, t.fg, 0.15);
                    } else if hot {
                        fill = if o.kind == Kind::Primary {
                            mix(fill, t.bg, 0.12)
                        } else {
                            t.hover(fill)
                        };
                    }
                    cv.fill_round(rf(full), radius, fill);
                    let tw = text_w(fonts.semi, &o.text);
                    let is = if o.icon.is_some() { p(18) } else { 0 };
                    let gap = if is > 0 { p(8) } else { 0 };
                    let x0 = (w - tw - is - gap) / 2;
                    if let Some(ic) = o.icon {
                        cv.icon(ic, x0 as f32, ((h - is) / 2) as f32, is as f32, fg);
                    }
                    texts.push((
                        rect(x0 + is + gap, 0, tw + p(4), h),
                        o.text.clone(),
                        fonts.semi,
                        fg,
                        DT_SINGLELINE | DT_VCENTER,
                    ));
                }
                Kind::Toggle => {
                    if hot {
                        cv.fill_round(rf(full), radius, t.hover(bg));
                    }
                    let (sw, sh) = (p(44), p(24));
                    let sx = w - p(14) - sw;
                    let sy = (h - sh) / 2;
                    let track = if o.on { t.primary } else { t.track };
                    cv.fill_round(rf(rect(sx, sy, sw, sh)), sh as f32 / 2.0, track);
                    let kr = (sh - p(6)) as f32 / 2.0;
                    let kx = if o.on {
                        (sx + sw - p(3)) as f32 - kr
                    } else {
                        (sx + p(3)) as f32 + kr
                    };
                    let knob = if o.on {
                        t.on_primary
                    } else if t.dark {
                        0xD4D4D8
                    } else {
                        0xFFFFFF
                    };
                    cv.fill_circle(kx, (sy + sh / 2) as f32, kr, argb(knob));
                    let tw = sx - p(16) - p(14);
                    let th = text_h(fonts.semi, &o.text, tw);
                    let dh = if o.desc.is_empty() {
                        0
                    } else {
                        p(3) + text_h(fonts.small, &o.desc, tw)
                    };
                    let y0 = (h - th - dh) / 2;
                    texts.push((
                        rect(p(14), y0, tw, th),
                        o.text.clone(),
                        fonts.semi,
                        t.fg,
                        DT_WORDBREAK | DT_EDITCONTROL,
                    ));
                    if dh > 0 {
                        let dr = rect(p(14), y0 + th + p(3), tw, dh);
                        texts.push((dr, o.desc.clone(), fonts.small, t.muted, DT_WORDBREAK | DT_EDITCONTROL));
                    }
                }
                Kind::Select(list) => {
                    cv.fill_round(rf(inset(full, 0)), radius, t.field);
                    let border = if hot || pressed { mix(t.border, t.primary, 0.6) } else { t.border };
                    cv.stroke_round(rf(inset(full, 1)), radius, argb(border), p(1).max(1) as f32);
                    let is = p(18);
                    cv.icon(Icon::Chevron, (w - p(12) - is) as f32, ((h - is) / 2) as f32, is as f32, t.muted);
                    let label = list.get(o.sel).map(|c| c.1).unwrap_or("");
                    let r = rect(p(14), 0, w - p(14) - is - p(16), h);
                    texts.push((r, label.to_string(), fonts.body, t.fg, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS));
                }
                Kind::Rule(i) => {
                    let sel = cur == Some(i);
                    if sel {
                        cv.fill_round(rf(full), radius, t.soft);
                    } else if hot {
                        cv.fill_round(rf(full), radius, t.hover(bg));
                    }
                    let rule = self.st.borrow().rules.get(i).cloned().unwrap_or_default();
                    let title = rule_title(&rule).to_string();
                    let detail = rule_detail(&rule);
                    let tw = w - p(28);
                    let th = text_h(fonts.semi, &title, tw);
                    let dh = text_h(fonts.small, &detail, tw);
                    let y0 = (h - th - dh - p(2)) / 2;
                    let tc = if sel { t.primary } else { t.fg };
                    texts.push((rect(p(14), y0, tw, th), title, fonts.semi, tc, DT_SINGLELINE | DT_END_ELLIPSIS));
                    let dr = rect(p(14), y0 + th + p(2), tw, dh);
                    texts.push((dr, detail, fonts.small, t.muted, DT_SINGLELINE | DT_END_ELLIPSIS));
                }
            }
            if focus {
                cv.stroke_round(rf(inset(full, 1)), (radius - 1.0).max(1.0), argb(t.primary), p(2) as f32);
            }
        }
        for (r, s, f, c, fl) in texts {
            draw_text(buf.dc, f, &s, r, c, fl);
        }
    }

    /// Choix dans une liste déroulante : menu de Windows sous le contrôle.
    fn open_select(&self, h: HWND) {
        let Some(Kind::Select(list)) = self.owners.borrow().get(&(h.0 as isize)).map(|o| o.kind) else {
            return;
        };
        let cur = self.sel(h);
        let chosen = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            for (i, (_, label)) in list.iter().enumerate() {
                let t = wide(label);
                let flags = if i == cur { MF_STRING | MF_CHECKED } else { MF_STRING };
                let _ = AppendMenuW(menu, flags, i + 1, PCWSTR(t.as_ptr()));
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            let id = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
                r.left,
                r.bottom + self.px(4),
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            id.0 as usize
        };
        if chosen > 0 {
            self.set_sel(h, chosen - 1);
            self.select_changed(h);
        }
    }

    fn select_changed(&self, h: HWND) {
        if h == self.c.s_season {
            let season = self.sel_value(h, SEASONS);
            let changed = {
                let mut st = self.st.borrow_mut();
                match st.cur {
                    Some(i) if i < st.rules.len() => {
                        st.rules[i].season = season;
                        true
                    }
                    _ => false,
                }
            };
            if changed {
                self.refresh_rule_buttons();
            }
        }
    }

    // ---------- mise en page ----------

    /// Rectangles de la fenêtre principale : barre latérale, contenu, barre du bas.
    fn frame(&self) -> (RECT, RECT, RECT) {
        let rc = client_rect(self.hwnd);
        let s = self.px(SIDEBAR_W).min(rc.right);
        let f = self.px(FOOTER_H);
        (
            rect(0, 0, s, rc.bottom),
            rect(s, 0, rc.right - s, rc.bottom - f),
            rect(s, rc.bottom - f, rc.right - s, f),
        )
    }

    fn footer_buttons(&self) -> (RECT, RECT, RECT) {
        let (_, _, f) = self.frame();
        let p = |v| self.px(v);
        let y = f.top + (f.bottom - f.top - p(BTN_H)) / 2;
        let save = rect(f.right - p(24) - p(136), y, p(136), p(BTN_H));
        let close = rect(save.left - p(10) - p(112), y, p(112), p(BTN_H));
        let next = rect(f.left + p(24), y, p(176), p(BTN_H));
        (next, close, save)
    }

    fn layout(&self) {
        if !self.ready.get() {
            return;
        }
        let (side, content, _) = self.frame();
        let p = |v| self.px(v);
        for (i, h) in self.c.nav.iter().enumerate() {
            move_win(*h, p(12), p(96) + i as i32 * p(46), side.right - p(24), p(42));
        }
        let (next, close, save) = self.footer_buttons();
        move_rect(self.c.b_next, next);
        move_rect(self.c.b_close, close);
        move_rect(self.c.b_save, save);
        move_rect(self.c.content, content);
        invalidate(self.hwnd);
        self.layout_content();
    }

    /// Place les contrôles de la page affichée et prépare ce qu'elle dessine.
    fn layout_content(&self) {
        if !self.ready.get() || self.in_layout.replace(true) {
            return;
        }
        // Deux passes au plus : l'apparition de la barre de défilement réduit la largeur.
        for _ in 0..2 {
            let rc = client_rect(self.c.content);
            let page = self.st.borrow().page;
            let total = self.layout_page(page, rc);
            let scroll = {
                let mut st = self.st.borrow_mut();
                st.total = total;
                st.scroll = st.scroll.clamp(0, (total - rc.bottom).max(0));
                st.scroll
            };
            let si = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: (total - 1).max(0),
                nPage: rc.bottom.max(0) as u32,
                nPos: scroll,
                nTrackPos: 0,
            };
            unsafe {
                SetScrollInfo(self.c.content, SB_VERT, &si, true);
            }
            if client_rect(self.c.content).right == rc.right {
                break;
            }
        }
        // Le défilement a pu être réduit : on replace avec la bonne position.
        let rc = client_rect(self.c.content);
        let page = self.st.borrow().page;
        self.layout_page(page, rc);
        self.in_layout.set(false);
        unsafe {
            let _ = RedrawWindow(Some(self.c.content), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
    }

    /// Page `page` dans la zone `rc` ; renvoie la hauteur totale de son contenu.
    fn layout_page(&self, page: usize, rc: RECT) -> i32 {
        let p = |v| self.px(v);
        let c = &self.c;
        let scroll = self.st.borrow().scroll;
        let full_w = rc.right - 2 * p(PAD);
        let mut f = Flow {
            w: self,
            x: p(PAD),
            y: p(28) - scroll,
            cw: full_w.min(p(FORM_W)),
            draws: vec![],
            card: None,
            rows: 0,
        };
        let (http_warn, excluded, n_selected, test_msg, has_rule, n_rules) = {
            let st = self.st.borrow();
            (
                st.http_warn,
                st.excluded,
                st.selected.len(),
                st.test_msg.clone(),
                st.cur.is_some(),
                st.rules.len(),
            )
        };
        let mut fixed = false; // page à la hauteur de la fenêtre (grille d'albums)
        match page {
            PAGE_SERVER => {
                f.header("Serveur", "Connexion à ton serveur Immich.");
                f.begin_card();
                f.field("URL du serveur", c.e_url, "", if http_warn { HTTP_WARN } else { "" });
                f.field("Token (clé API)", c.e_key, KEY_HINT, "");
                f.end_card();
                f.begin_card();
                f.toggle(c.t_insecure);
                f.end_card();
                let bw = p(214);
                move_win(c.b_test, f.x, f.y, bw, p(BTN_H));
                self.set_bg(c.b_test, Bg::Page);
                if !test_msg.0.is_empty() {
                    let tw = f.cw - bw - p(16);
                    let th = text_h(self.fonts.borrow().body, &test_msg.0, tw);
                    let y = f.y + (p(BTN_H) - th).max(0) / 2;
                    f.text(rect(f.x + bw + p(16), y, tw, th), &test_msg.0, Font::Body, test_msg.1);
                    f.y += th.max(p(BTN_H));
                } else {
                    f.y += p(BTN_H);
                }
            }
            PAGE_ALBUMS => {
                fixed = true;
                f.cw = full_w;
                f.header(
                    "Albums",
                    &format!("{} — les photos sont tirées au hasard dans ces albums.", count_label(n_selected)),
                );
                let fw = p(320).min(f.cw / 2);
                let field = rect(f.x, f.y, fw, p(FIELD_H));
                f.place_field(field, c.e_filter, p(26));
                f.draws.push(Draw::Icon(
                    rect(field.left + p(12), field.top + (p(FIELD_H) - p(18)) / 2, p(18), p(18)),
                    Icon::Search,
                    Tone::Muted,
                ));
                let bw = p(150);
                move_win(c.b_reload, f.x + f.cw - bw, f.y + (p(FIELD_H) - p(BTN_H)) / 2, bw, p(BTN_H));
                self.set_bg(c.b_reload, Bg::Page);
                if !test_msg.0.is_empty() {
                    let mr = rect(field.right + p(16), f.y, f.cw - fw - bw - p(32), p(FIELD_H));
                    let flags = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;
                    f.draws.push(Draw::Text(mr, test_msg.0.clone(), Font::Body, test_msg.1, flags));
                }
                f.y += p(FIELD_H) + p(20);
                let gh = (rc.bottom - f.y - p(20)).max(p(160));
                move_win(c.grid_albums, f.x, f.y, f.cw, gh);
                f.y += gh;
            }
            PAGE_DISPLAY => {
                f.header("Affichage", "Fréquence, cadrage et qualité des photos.");
                f.begin_card();
                f.number_row("Changer d'image toutes les", "", c.e_interval, "minutes");
                f.select_row("Ajustement", "Comment la photo remplit l'écran.", c.s_style);
                f.select_row(
                    "Qualité",
                    "Pleine résolution : JPEG généré par Immich (HEIC et RAW compris).",
                    c.s_size,
                );
                f.select_row("Plusieurs écrans", "", c.s_monitor);
                f.number_row("Photos gardées hors ligne", OFFLINE_HINT, c.e_offline, "");
                f.end_card();
                f.begin_card();
                for t in [c.t_split, c.t_landscape, c.t_start, c.t_unlock] {
                    f.toggle(t);
                }
                f.end_card();
                show_win(c.b_show_excluded, excluded > 0);
                if excluded > 0 {
                    f.begin_card();
                    let bw = p(150);
                    let (top, h, x) = f.row(
                        &format!("{excluded} photo(s) masquée(s)"),
                        "Depuis le menu « Ne plus afficher cette photo ».",
                        bw,
                    );
                    move_win(c.b_show_excluded, x, top + (h - p(BTN_H)) / 2, bw, p(BTN_H));
                    self.set_bg(c.b_show_excluded, Bg::Card);
                    f.end_card();
                }
            }
            PAGE_SCHEDULE => {
                f.header("Planification", "Quand l'image change toute seule.");
                f.begin_card();
                let (top, h, x) = f.row("Plage horaire", HOURS_HINT, p(88) * 2 + p(32));
                f.hours(top, h, x, c.e_from, c.e_to);
                f.end_card();
                f.begin_card();
                for t in [c.t_battery, c.t_metered, c.t_fullscreen] {
                    f.toggle(t);
                }
                f.end_card();
                f.y -= p(8);
                f.note(PAUSES_HINT, Tone::Muted);
                f.begin_card();
                f.toggle(c.t_preload);
                f.end_card();
            }
            PAGE_RULES => {
                fixed = true;
                f.cw = full_w;
                f.header("Saisons et heures", RULES_HINT);
                let top = f.y;
                let bottom = (rc.bottom - p(24)).max(top + p(300));
                // Liste des règles.
                let lw = p(RULES_W);
                let list = rect(f.x, top, lw, bottom - top);
                f.draws.push(Draw::Card(list));
                let btns = self.st.borrow().rule_btns.clone();
                let mut y = top + p(10);
                for b in &btns {
                    move_win(*b, list.left + p(8), y, lw - p(16), p(58));
                    self.set_bg(*b, Bg::Card);
                    y += p(62);
                }
                if n_rules == 0 {
                    let r = rect(list.left + p(20), y + p(6), lw - p(40), p(60));
                    f.text(r, "Aucune règle pour l'instant.", Font::Small, Tone::Muted);
                }
                move_win(
                    c.b_add_rule,
                    list.left + p(12),
                    list.bottom - p(12) - p(BTN_H),
                    lw - p(24),
                    p(BTN_H),
                );
                self.set_bg(c.b_add_rule, Bg::Card);
                // Règle sélectionnée.
                let ex = list.right + p(16);
                let ed = rect(ex, top, f.x + f.cw - ex, bottom - top);
                f.draws.push(Draw::Card(ed));
                for h in [c.b_del_rule, c.s_season, c.e_rfrom, c.e_rto, c.grid_rule] {
                    show_win(h, has_rule && page == PAGE_RULES);
                }
                let (ix, iw) = (ed.left + p(20), ed.right - ed.left - p(40));
                if has_rule {
                    let mut y = ed.top + p(18);
                    let semi = self.fonts.borrow().semi;
                    let lab = |f: &mut Flow, y: i32, s: &str| {
                        let r = rect(ix, y, p(90), p(FIELD_H));
                        f.draws
                            .push(Draw::Text(r, s.to_string(), Font::Semi, Tone::Fg, DT_SINGLELINE | DT_VCENTER));
                    };
                    // En-tête : règle sélectionnée et suppression.
                    let bw = p(140);
                    move_win(c.b_del_rule, ed.right - p(20) - bw, y, bw, p(BTN_H));
                    self.set_bg(c.b_del_rule, Bg::Card);
                    let title = self
                        .st
                        .borrow()
                        .cur
                        .and_then(|i| self.st.borrow().rules.get(i).map(|r| rule_title(r).to_string()));
                    let hr = rect(ix, y, iw - bw - p(16), p(BTN_H));
                    let flags = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;
                    f.draws.push(Draw::Text(
                        hr,
                        format!("Règle : {}", title.unwrap_or_default()),
                        Font::Semi,
                        Tone::Muted,
                        flags,
                    ));
                    y += p(BTN_H) + p(14);
                    lab(&mut f, y, "Saison");
                    move_win(c.s_season, ix + p(100), y, p(260).min(iw - p(100)), p(FIELD_H));
                    self.set_bg(c.s_season, Bg::Card);
                    y += p(FIELD_H) + p(12);
                    lab(&mut f, y, "Heures");
                    f.hours(y, p(FIELD_H), ix + p(100), c.e_rfrom, c.e_rto);
                    y += p(FIELD_H) + p(20);
                    let lh = text_h(semi, "Albums", iw);
                    f.text(rect(ix, y, iw, lh), "Albums de cette règle", Font::Semi, Tone::Fg);
                    y += lh + p(10);
                    move_win(c.grid_rule, ix, y, iw, (ed.bottom - p(16) - y).max(p(120)));
                } else {
                    let msg = if n_rules == 0 {
                        "Ajoute une règle pour utiliser d'autres albums selon la saison ou l'heure."
                    } else {
                        "Choisis une règle à gauche."
                    };
                    let mh = text_h(self.fonts.borrow().body, msg, iw);
                    let r = rect(ix, ed.top + (ed.bottom - ed.top - mh) / 2, iw, mh);
                    f.draws.push(Draw::Text(
                        r,
                        msg.into(),
                        Font::Body,
                        Tone::Muted,
                        DT_WORDBREAK | windows::Win32::Graphics::Gdi::DT_CENTER,
                    ));
                }
                f.y = bottom;
            }
            PAGE_CAPTION => {
                f.header("Légende", "Lieu et date incrustés sur la photo, dans une carte façon verre dépoli.");
                f.begin_card();
                f.toggle(c.t_place);
                f.toggle(c.t_date);
                f.select_row("Position du texte", "", c.s_caption);
                f.end_card();
                f.begin_card();
                f.toggle(c.t_link);
                f.end_card();
                f.begin_card();
                f.toggle(c.t_lock);
                f.end_card();
            }
            _ => {
                let version = if self.portable {
                    format!("Version {VERSION} (portable)")
                } else {
                    format!("Version {VERSION}")
                };
                f.header("Mises à jour", &version);
                f.begin_card();
                f.toggle(c.t_update);
                f.end_card();
            }
        }
        *self.draws.borrow_mut() = f.draws;
        if fixed {
            rc.bottom
        } else {
            f.y + scroll + p(PAD)
        }
    }

    fn show_page(&self, i: usize) {
        let old = {
            let mut st = self.st.borrow_mut();
            let old = st.page;
            st.page = i;
            if old != i {
                st.scroll = 0;
            }
            old
        };
        let rule_btns = self.st.borrow().rule_btns.clone();
        for (j, list) in self.pages.iter().enumerate() {
            for h in list {
                show_win(*h, j == i);
            }
        }
        for b in rule_btns {
            show_win(b, i == PAGE_RULES);
        }
        invalidate(self.c.nav[old]);
        invalidate(self.c.nav[i]);
        // Le clavier ne doit pas rester sur un contrôle désormais caché.
        unsafe {
            let f = GetFocus();
            if !f.is_invalid() && IsChild(self.c.content, f).as_bool() && !IsWindowVisible(f).as_bool() {
                let _ = SetFocus(Some(self.c.nav[i]));
            }
        }
        self.layout_content();
    }

    /// Fait défiler la page pour montrer le contrôle qui reçoit le focus.
    fn ensure_visible(&self, h: HWND) {
        unsafe {
            if !IsChild(self.c.content, h).as_bool() || self.grid_of(h).is_some() {
                return;
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            let mut pt = POINT { x: r.left, y: r.top };
            let _ = ScreenToClient(self.c.content, &mut pt);
            let hgt = r.bottom - r.top;
            let view = client_rect(self.c.content).bottom;
            let m = self.px(16);
            let delta = if pt.y < m {
                pt.y - m
            } else if pt.y + hgt > view - m {
                pt.y + hgt - view + m
            } else {
                0
            };
            if delta != 0 {
                let cur = self.st.borrow().scroll;
                self.scroll_to(cur + delta);
            }
        }
    }

    fn scroll_to(&self, y: i32) {
        let changed = {
            let mut st = self.st.borrow_mut();
            let max = (st.total - client_rect(self.c.content).bottom).max(0);
            let y = y.clamp(0, max);
            let changed = y != st.scroll;
            st.scroll = y;
            changed
        };
        if changed {
            self.layout_content();
        }
    }

    // ---------- peinture ----------

    fn paint_main(&self, dc: HDC) {
        let t = self.t();
        let p = |v| self.px(v);
        let rc = client_rect(self.hwnd);
        let (side, _, foot) = self.frame();
        let buf = Buffer::new(dc, rc);
        buf.fill(rc, t.bg);
        buf.fill(rect(side.right - p(1).max(1), 0, p(1).max(1), rc.bottom), t.divider);
        buf.fill(rect(foot.left, foot.top, foot.right - foot.left, p(1).max(1)), t.divider);
        let fonts = self.fonts.borrow();
        // Marque.
        let brand = self.icons.get().2;
        if !brand.is_invalid() {
            unsafe {
                let _ = DrawIconEx(buf.dc, p(22), p(26), brand, p(32), p(32), 0, None, DI_NORMAL);
            }
        }
        draw_text(
            buf.dc,
            fonts.brand,
            "Immich Wallpaper",
            rect(p(64), p(24), side.right - p(72), p(22)),
            t.fg,
            DT_SINGLELINE,
        );
        let ver = if self.portable {
            format!("{VERSION} · portable")
        } else {
            VERSION.to_string()
        };
        draw_text(
            buf.dc,
            fonts.small,
            &ver,
            rect(p(64), p(46), side.right - p(72), p(18)),
            t.muted,
            DT_SINGLELINE,
        );
        // État de l'appli, en bas de la barre latérale.
        let status = self.st.borrow().last_status.clone();
        if !status.is_empty() {
            let wd = side.right - p(44);
            let h = text_h(fonts.small, &status, wd).min(p(80));
            let y = rc.bottom - p(22) - h;
            {
                let cv = Canvas::new(buf.dc);
                cv.fill_circle(p(26) as f32, (y + p(8)) as f32, p(3) as f32, argb(t.primary));
            }
            draw_text(
                buf.dc,
                fonts.small,
                &status,
                rect(p(36), y, wd - p(4), h),
                t.muted,
                DT_WORDBREAK | DT_EDITCONTROL,
            );
        }
        // Message de la barre du bas.
        let (msg, tone) = self.st.borrow().msg.clone();
        if !msg.is_empty() {
            let (next, close, _) = self.footer_buttons();
            let r = rect(
                next.right + p(16),
                foot.top,
                close.left - p(16) - next.right - p(16),
                foot.bottom - foot.top,
            );
            let h = text_h(fonts.body, &msg, r.right - r.left).min(r.bottom - r.top);
            let r2 = rect(r.left, r.top + (r.bottom - r.top - h) / 2, r.right - r.left, h);
            draw_text(
                buf.dc,
                fonts.body,
                &msg,
                r2,
                t.tone(tone),
                DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS,
            );
        }
    }

    fn paint_content(&self, dc: HDC) {
        let t = self.t();
        let rc = client_rect(self.c.content);
        let buf = Buffer::new(dc, rc);
        buf.fill(rc, t.bg);
        let focus = unsafe { GetFocus() };
        let draws = self.draws.borrow();
        let p = |v| self.px(v);
        {
            let cv = Canvas::new(buf.dc);
            for d in draws.iter() {
                match d {
                    Draw::Card(r) => cv.fill_round(rf(*r), p(18) as f32, t.card),
                    Draw::Field(r, e) => {
                        let rr = rf(*r);
                        cv.fill_round(rr, p(10) as f32, t.field);
                        let (color, wd) = if *e == focus { (t.primary, p(2)) } else { (t.border, p(1).max(1)) };
                        let half = wd as f32 / 2.0;
                        let br = Rf {
                            x: rr.x + half,
                            y: rr.y + half,
                            w: rr.w - wd as f32,
                            h: rr.h - wd as f32,
                        };
                        cv.stroke_round(br, p(10) as f32 - half, argb(color), wd as f32);
                    }
                    Draw::Divider(r) => buf.fill(*r, t.divider),
                    Draw::Icon(r, ic, tone) => cv.icon(*ic, r.left as f32, r.top as f32, (r.right - r.left) as f32, t.tone(*tone)),
                    Draw::Text(..) => {}
                }
            }
        }
        let fonts = self.fonts.borrow();
        for d in draws.iter() {
            if let Draw::Text(r, s, f, tone, flags) = d {
                draw_text(buf.dc, fonts.get(*f), s, *r, t.tone(*tone), *flags);
            }
        }
    }

    // ---------- grilles d'albums ----------

    fn geom(&self, i: usize, width: i32) -> Geom {
        let (min, gap) = if i == GRID_ALBUMS {
            (self.px(160), self.px(18))
        } else {
            (self.px(116), self.px(12))
        };
        let cols = ((width + gap) / (min + gap)).max(1) as usize;
        let cell_w = ((width - gap * (cols as i32 - 1)) / cols as i32).max(self.px(60));
        Geom {
            cols,
            cell_w,
            cell_h: cell_w + self.px(52),
            gap,
        }
    }

    fn grid_checked(&self, i: usize) -> Vec<String> {
        let st = self.st.borrow();
        if i == GRID_ALBUMS {
            st.selected.clone()
        } else {
            st.cur.and_then(|c| st.rules.get(c)).map(|r| r.albums.clone()).unwrap_or_default()
        }
    }

    /// Recalcule les albums affichés dans une grille (filtre compris).
    fn refill_grid(&self, i: usize) {
        let checked = self.grid_checked(i);
        let filter = if i == GRID_ALBUMS {
            get_text(self.c.e_filter)
        } else {
            String::new()
        };
        let rows: Vec<AlbumRow> = {
            let st = self.st.borrow();
            album_rows(&st.albums, &checked)
                .into_iter()
                .filter(|r| filter.trim().is_empty() || matches_filter(&r.name, &filter))
                .collect()
        };
        {
            let mut g = self.grids[i].borrow_mut();
            g.focus = g.focus.min(rows.len().saturating_sub(1));
            g.hot = None;
            g.rows = rows;
        }
        self.grid_update_scroll(i);
        invalidate(self.grid_hwnd(i));
    }

    fn grid_update_scroll(&self, i: usize) {
        let h = self.grid_hwnd(i);
        for _ in 0..2 {
            let rc = client_rect(h);
            let geo = self.geom(i, rc.right);
            let total = {
                let mut g = self.grids[i].borrow_mut();
                let lines = g.rows.len().div_ceil(geo.cols) as i32;
                g.total = (lines * (geo.cell_h + geo.gap) - geo.gap).max(0);
                g.scroll = g.scroll.clamp(0, (g.total - rc.bottom).max(0));
                (g.total, g.scroll)
            };
            let si = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: (total.0 - 1).max(0),
                nPage: rc.bottom.max(0) as u32,
                nPos: total.1,
                nTrackPos: 0,
            };
            unsafe {
                SetScrollInfo(h, SB_VERT, &si, true);
            }
            if client_rect(h).right == rc.right {
                break;
            }
        }
    }

    fn grid_scroll_to(&self, i: usize, y: i32) {
        let view = client_rect(self.grid_hwnd(i)).bottom;
        {
            let mut g = self.grids[i].borrow_mut();
            g.scroll = y.clamp(0, (g.total - view).max(0));
        }
        self.grid_update_scroll(i);
        invalidate(self.grid_hwnd(i));
    }

    fn cell_rect(&self, _i: usize, idx: usize, geo: &Geom, scroll: i32) -> RECT {
        let (col, line) = ((idx % geo.cols) as i32, (idx / geo.cols) as i32);
        rect(
            col * (geo.cell_w + geo.gap),
            line * (geo.cell_h + geo.gap) - scroll,
            geo.cell_w,
            geo.cell_h,
        )
    }

    fn grid_hit(&self, i: usize, x: i32, y: i32) -> Option<usize> {
        let rc = client_rect(self.grid_hwnd(i));
        let geo = self.geom(i, rc.right);
        let g = self.grids[i].borrow();
        (0..g.rows.len()).find(|&k| {
            let r = self.cell_rect(i, k, &geo, g.scroll);
            x >= r.left && x < r.right && y >= r.top && y < r.top + geo.cell_w
        })
    }

    /// Coche ou décoche l'album `idx` de la grille.
    fn grid_toggle(&self, i: usize, idx: usize) {
        let Some(id) = self.grids[i].borrow().rows.get(idx).map(|r| r.id.clone()) else {
            return;
        };
        {
            let mut st = self.st.borrow_mut();
            if i == GRID_ALBUMS {
                let on = !st.selected.contains(&id);
                toggle_id(&mut st.selected, &id, on);
            } else if let Some(c) = st.cur {
                if let Some(r) = st.rules.get_mut(c) {
                    let on = !r.albums.contains(&id);
                    toggle_id(&mut r.albums, &id, on);
                }
            }
        }
        self.grids[i].borrow_mut().focus = idx;
        invalidate(self.grid_hwnd(i));
        if i == GRID_ALBUMS {
            self.layout_content(); // nombre d'albums dans l'en-tête
        } else {
            self.refresh_rule_buttons();
        }
    }

    fn grid_ensure_visible(&self, i: usize, idx: usize) {
        let rc = client_rect(self.grid_hwnd(i));
        let geo = self.geom(i, rc.right);
        let scroll = self.grids[i].borrow().scroll;
        let r = self.cell_rect(i, idx, &geo, scroll);
        if r.top < 0 {
            self.grid_scroll_to(i, scroll + r.top);
        } else if r.bottom > rc.bottom {
            self.grid_scroll_to(i, scroll + r.bottom - rc.bottom);
        }
    }

    fn grid_key(&self, i: usize, vk: usize) -> bool {
        let rc = client_rect(self.grid_hwnd(i));
        let geo = self.geom(i, rc.right);
        let (n, focus) = {
            let g = self.grids[i].borrow();
            (g.rows.len(), g.focus)
        };
        if n == 0 {
            return false;
        }
        let page = ((rc.bottom / (geo.cell_h + geo.gap)).max(1) as usize) * geo.cols;
        let next = match vk {
            VK_LEFT => focus.saturating_sub(1),
            VK_RIGHT => (focus + 1).min(n - 1),
            VK_UP => focus.saturating_sub(geo.cols),
            VK_DOWN => (focus + geo.cols).min(n - 1),
            VK_PRIOR => focus.saturating_sub(page),
            VK_NEXT => (focus + page).min(n - 1),
            VK_HOME => 0,
            VK_END => n - 1,
            _ => return false,
        };
        self.grids[i].borrow_mut().focus = next;
        self.grid_ensure_visible(i, next);
        invalidate(self.grid_hwnd(i));
        true
    }

    fn paint_grid(&self, i: usize, dc: HDC) {
        let t = self.t();
        let p = |v| self.px(v);
        let h = self.grid_hwnd(i);
        let rc = client_rect(h);
        let bg = if i == GRID_ALBUMS { t.bg } else { t.card };
        let buf = Buffer::new(dc, rc);
        buf.fill(rc, bg);
        let geo = self.geom(i, rc.right);
        let checked = self.grid_checked(i);
        let server = self.st.borrow().cover_server.clone();
        let has_focus = unsafe { GetFocus() } == h;
        let fonts = self.fonts.borrow();
        let g = self.grids[i].borrow();
        if g.rows.is_empty() {
            let has_albums = !self.st.borrow().albums.is_empty();
            let msg = if has_albums {
                "Aucun album ne correspond."
            } else {
                "Teste la connexion (page Serveur) pour afficher tes albums."
            };
            let r = rect(p(16), rc.bottom / 2 - p(30), rc.right - p(32), p(60));
            draw_text(
                buf.dc,
                fonts.body,
                msg,
                r,
                t.muted,
                DT_WORDBREAK | windows::Win32::Graphics::Gdi::DT_CENTER,
            );
            return;
        }
        let mut texts = vec![];
        {
            let cv = Canvas::new(buf.dc);
            for (k, row) in g.rows.iter().enumerate() {
                let cell = self.cell_rect(i, k, &geo, g.scroll);
                if cell.bottom < 0 || cell.top > rc.bottom {
                    continue;
                }
                let on = checked.contains(&row.id);
                let hot = g.hot == Some(k);
                let thumb = rect(cell.left, cell.top, geo.cell_w, geo.cell_w);
                let radius = p(14) as f32;
                if on {
                    cv.fill_round(rf(thumb), radius, t.soft);
                }
                let img = if on { inset(thumb, p(10)) } else { thumb };
                match (!row.thumb.is_empty()).then(|| cached_cover(&server, &row.thumb)).flatten() {
                    Some(cover) => cv.image_round(&cover, rf(img), if on { p(10) as f32 } else { radius }),
                    None => {
                        cv.fill_round(
                            rf(img),
                            if on { p(10) as f32 } else { radius },
                            if t.dark { 0x2C2C2C } else { 0xE9E9E6 },
                        );
                        let is = (geo.cell_w / 4).max(p(24));
                        let ix = img.left + (img.right - img.left - is) / 2;
                        let iy = img.top + (img.bottom - img.top - is) / 2;
                        cv.icon(Icon::Albums, ix as f32, iy as f32, is as f32, t.muted);
                    }
                }
                let (cx, cy, cr) = ((thumb.left + p(20)) as f32, (thumb.top + p(20)) as f32, p(11) as f32);
                if on {
                    cv.fill_circle(cx, cy, cr, argb(t.primary));
                    let is = p(14) as f32;
                    cv.icon(Icon::Check, cx - is / 2.0, cy - is / 2.0, is, t.on_primary);
                } else if hot {
                    cv.fill_circle(cx, cy, cr, 0x5A00_0000);
                    cv.stroke_circle(cx, cy, cr - 1.0, 0xFFFF_FFFF, p(2) as f32);
                }
                if has_focus && g.focus == k {
                    let fr = inset(thumb, -p(3));
                    cv.stroke_round(rf(fr), radius + p(3) as f32, argb(t.primary), p(2) as f32);
                }
                let ty = thumb.bottom + p(8);
                let name_r = rect(cell.left + p(2), ty, geo.cell_w - p(4), p(20));
                let count_r = rect(cell.left + p(2), ty + p(21), geo.cell_w - p(4), p(18));
                texts.push((name_r, row.name.clone(), fonts.semi, t.fg));
                texts.push((count_r, items_label(row.count), fonts.small, t.muted));
            }
        }
        for (r, s, f, c) in texts {
            draw_text(buf.dc, f, &s, r, c, DT_SINGLELINE | DT_END_ELLIPSIS);
        }
    }

    // ---------- messages ----------

    fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if let Some(i) = self.grid_of(hwnd) {
            return self.handle_grid(i, hwnd, msg, wp, lp);
        }
        match msg {
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_PAINT => {
                unsafe {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut ps);
                    if hwnd == self.hwnd {
                        self.paint_main(dc);
                    } else {
                        self.paint_content(dc);
                    }
                    let _ = EndPaint(hwnd, &ps);
                }
                return Some(LRESULT(0));
            }
            WM_COMMAND => {
                self.command(wp.0 & 0xFFFF, (wp.0 >> 16) as u32 & 0xFFFF, HWND(lp.0 as *mut _));
                return Some(LRESULT(0));
            }
            WM_DRAWITEM if lp.0 != 0 => {
                self.draw_item(unsafe { &*(lp.0 as *const DRAWITEMSTRUCT) });
                return Some(LRESULT(1));
            }
            WM_CTLCOLOREDIT => {
                let t = self.t();
                let dc = HDC(wp.0 as *mut _);
                unsafe {
                    SetTextColor(dc, colorref(t.fg));
                    SetBkColor(dc, colorref(t.field));
                }
                return Some(LRESULT(self.field_brush.get().0 as isize));
            }
            _ => {}
        }
        if hwnd == self.c.content {
            return match msg {
                WM_SIZE => {
                    if !self.in_layout.get() {
                        self.layout_content();
                    }
                    Some(LRESULT(0))
                }
                WM_MOUSEWHEEL => {
                    let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                    let cur = self.st.borrow().scroll;
                    self.scroll_to(cur - delta * self.px(64) / 120);
                    Some(LRESULT(0))
                }
                WM_VSCROLL => {
                    let cur = self.st.borrow().scroll;
                    let pos = self.scroll_pos(hwnd, wp, cur, self.px(48));
                    self.scroll_to(pos);
                    Some(LRESULT(0))
                }
                _ => None,
            };
        }
        // Fenêtre principale.
        match msg {
            WM_SIZE => self.layout(),
            WM_GETMINMAXINFO if lp.0 != 0 => {
                let mut r = RECT {
                    left: 0,
                    top: 0,
                    right: self.px(MIN_W),
                    bottom: self.px(MIN_H),
                };
                unsafe {
                    let _ = AdjustWindowRectExForDpi(
                        &mut r,
                        WINDOW_STYLE(WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN),
                        false,
                        WINDOW_EX_STYLE(WS_EX_CONTROLPARENT),
                        self.dpi.get(),
                    );
                    let mmi = &mut *(lp.0 as *mut MINMAXINFO);
                    mmi.ptMinTrackSize.x = r.right - r.left;
                    mmi.ptMinTrackSize.y = r.bottom - r.top;
                }
            }
            WM_ACTIVATE => unsafe {
                if wp.0 & 0xFFFF == 0 {
                    // Désactivée : on retient le contrôle actif.
                    let f = GetFocus();
                    if !f.is_invalid() && IsChild(self.hwnd, f).as_bool() {
                        self.st.borrow_mut().focus = f.0 as isize;
                    }
                } else {
                    let f = self.st.borrow().focus;
                    if f != 0 {
                        let _ = SetFocus(Some(hwnd_of(f)));
                    }
                }
            },
            WM_DPICHANGED if lp.0 != 0 => {
                self.dpi.set(((wp.0 & 0xFFFF) as u32).max(96));
                *self.fonts.borrow_mut() = Fonts::new(self.dpi.get());
                let body = self.fonts.borrow().body;
                for h in &self.edits {
                    send(*h, WM_SETFONT, body.0 as usize, 0);
                }
                self.set_icons();
                unsafe {
                    let r = *(lp.0 as *const RECT);
                    let _ = SetWindowPos(
                        self.hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                self.layout();
                self.refill_grid(GRID_ALBUMS);
                self.refill_grid(GRID_RULE);
            }
            WM_SETTINGCHANGE => {
                let dark = system_dark();
                if dark != self.t().dark {
                    self.apply_theme(if dark { DARK } else { LIGHT });
                }
                return None;
            }
            WM_TIMER => self.refresh_status(),
            WM_APP_DONE => self.take_done(),
            DM_GETDEFID => return Some(LRESULT((DC_HASDEFID << 16) | ID_SAVE as isize)),
            WM_CLOSE => self.close(),
            WM_DESTROY => unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_STATUS);
            },
            WM_NCDESTROY => {
                self.release();
                return None;
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    /// Nouvelle position après un message WM_VSCROLL.
    fn scroll_pos(&self, h: HWND, wp: WPARAM, cur: i32, line: i32) -> i32 {
        let view = client_rect(h).bottom;
        match (wp.0 & 0xFFFF) as i32 {
            0 => cur - line,   // SB_LINEUP
            1 => cur + line,   // SB_LINEDOWN
            2 => cur - view,   // SB_PAGEUP
            3 => cur + view,   // SB_PAGEDOWN
            6 => 0,            // SB_TOP
            7 => i32::MAX / 2, // SB_BOTTOM
            4 | 5 => {
                // SB_THUMBPOSITION, SB_THUMBTRACK
                let mut si = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    ..Default::default()
                };
                unsafe {
                    let _ = GetScrollInfo(h, SB_VERT, &mut si);
                }
                si.nTrackPos
            }
            _ => cur,
        }
    }

    fn handle_grid(&self, i: usize, h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_PAINT => unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                self.paint_grid(i, dc);
                let _ = EndPaint(h, &ps);
            },
            WM_SIZE => {
                self.grid_update_scroll(i);
                invalidate(h);
            }
            WM_GETDLGCODE => return Some(LRESULT(DLGC_WANTARROWS | DLGC_WANTCHARS)),
            WM_SETFOCUS | WM_KILLFOCUS => invalidate(h),
            WM_LBUTTONDOWN => {
                unsafe {
                    let _ = SetFocus(Some(h));
                }
                let (x, y) = ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32);
                if let Some(k) = self.grid_hit(i, x, y) {
                    self.grid_toggle(i, k);
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32);
                let hit = self.grid_hit(i, x, y);
                let changed = {
                    let mut g = self.grids[i].borrow_mut();
                    let changed = g.hot != hit;
                    g.hot = hit;
                    changed
                };
                if changed {
                    invalidate(h);
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: h,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                    }
                }
            }
            WM_MOUSELEAVE => {
                self.grids[i].borrow_mut().hot = None;
                invalidate(h);
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                let s = self.grids[i].borrow().scroll;
                self.grid_scroll_to(i, s - delta * self.px(80) / 120);
            }
            WM_VSCROLL => {
                let s = self.grids[i].borrow().scroll;
                let pos = self.scroll_pos(h, wp, s, self.px(60));
                self.grid_scroll_to(i, pos);
            }
            WM_KEYDOWN => {
                if !self.grid_key(i, wp.0) {
                    return None;
                }
            }
            WM_CHAR if wp.0 == 0x20 => {
                let f = self.grids[i].borrow().focus;
                self.grid_toggle(i, f);
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn command(&self, id: usize, code: u32, ctl: HWND) {
        let c = &self.c;
        if id == ID_SAVE && (code == BN_CLICKED || ctl.is_invalid()) {
            self.save();
            return;
        }
        if id == ID_CLOSE && (code == BN_CLICKED || ctl.is_invalid()) {
            self.close();
            return;
        }
        match code {
            EN_CHANGE => {
                if self.filling.get() {
                    return;
                }
                if ctl == c.e_url {
                    self.update_http_warn();
                } else if ctl == c.e_filter {
                    self.grids[GRID_ALBUMS].borrow_mut().scroll = 0;
                    self.refill_grid(GRID_ALBUMS);
                } else if ctl == c.e_rfrom || ctl == c.e_rto {
                    let (f, t) = (get_text(c.e_rfrom).trim().to_string(), get_text(c.e_rto).trim().to_string());
                    {
                        let mut st = self.st.borrow_mut();
                        if let Some(r) = st.cur.and_then(|i| st.rules.get_mut(i)) {
                            r.from = f;
                            r.to = t;
                        }
                    }
                    self.refresh_rule_buttons();
                }
            }
            EN_SETFOCUS | EN_KILLFOCUS => {
                if code == EN_SETFOCUS {
                    self.ensure_visible(ctl);
                }
                invalidate(c.content);
            }
            BN_SETFOCUS => self.ensure_visible(ctl),
            BN_CLICKED | BN_DOUBLECLICKED => {
                let kind = self.owners.borrow().get(&(ctl.0 as isize)).map(|o| o.kind);
                match kind {
                    Some(Kind::Toggle) => {
                        let on = !self.on(ctl);
                        self.set_on(ctl, on);
                    }
                    _ if code == BN_DOUBLECLICKED => {}
                    Some(Kind::Nav(i)) => self.show_page(i),
                    Some(Kind::Select(_)) => self.open_select(ctl),
                    Some(Kind::Rule(i)) => {
                        self.st.borrow_mut().cur = Some(i);
                        self.load_rule();
                    }
                    _ if ctl == c.b_test || ctl == c.b_reload => self.test(false),
                    _ if ctl == c.b_next => self.next(),
                    _ if ctl == c.b_show_excluded => self.clear_excluded(),
                    _ if ctl == c.b_add_rule => {
                        {
                            let mut st = self.st.borrow_mut();
                            st.rules.push(AlbumRule::default());
                            st.cur = Some(st.rules.len() - 1);
                        }
                        self.fill_rules();
                        unsafe {
                            let _ = SetFocus(Some(c.s_season));
                        }
                    }
                    _ if ctl == c.b_del_rule => {
                        {
                            let mut st = self.st.borrow_mut();
                            if let Some(i) = st.cur.filter(|&i| i < st.rules.len()) {
                                st.rules.remove(i);
                                st.cur = if st.rules.is_empty() {
                                    None
                                } else {
                                    Some(i.min(st.rules.len() - 1))
                                };
                            }
                        }
                        self.fill_rules();
                        unsafe {
                            let _ = SetFocus(Some(c.b_add_rule));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // ---------- contenu ----------

    fn load(&self) {
        let ui = self.settings.load();
        let cfg = &ui.cfg;
        let c = &self.c;
        self.filling.set(true);
        set_text(c.e_url, &cfg.server_url);
        self.set_on(c.t_insecure, cfg.insecure_skip_verify);

        let interval = if cfg.interval_minutes < 1 { 60 } else { cfg.interval_minutes };
        set_text(c.e_interval, &interval.to_string());
        self.set_sel(c.s_style, choice_index(STYLES, &cfg.style));
        self.set_sel(c.s_size, choice_index(SIZES, &cfg.image_size));
        self.set_sel(c.s_monitor, choice_index(MONITORS, &cfg.multi_monitor));
        set_text(c.e_offline, &cfg.offline_cache.clamp(0, 500).to_string());
        self.set_on(c.t_split, cfg.split_wide);
        self.set_on(c.t_landscape, cfg.landscape_only);
        self.set_on(c.t_start, cfg.change_on_start);
        self.set_on(c.t_unlock, cfg.change_on_unlock);

        set_text(c.e_from, &cfg.active_from);
        set_text(c.e_to, &cfg.active_to);
        self.set_on(c.t_battery, cfg.pause_on_battery);
        self.set_on(c.t_metered, cfg.pause_on_metered);
        self.set_on(c.t_fullscreen, cfg.pause_on_fullscreen);
        self.set_on(c.t_preload, cfg.preload);

        self.set_on(c.t_place, cfg.show_place);
        self.set_on(c.t_date, cfg.show_date);
        self.set_sel(c.s_caption, choice_index(CAPTIONS, &cfg.caption_position));
        self.set_on(c.t_link, cfg.desktop_link);
        self.set_on(c.t_lock, cfg.lock_screen);

        self.set_on(c.t_update, cfg.auto_update);
        if self.portable {
            self.set_owner(c.t_update, |o| {
                o.text = "Vérifier chaque jour si une nouvelle version existe".into();
                o.desc = format!("{UPDATE_PORTABLE_HINT}\n{UPDATE_HINT}");
            });
        }
        self.filling.set(false);

        {
            let mut st = self.st.borrow_mut();
            st.has_key = ui.has_key;
            st.cert_fp = cfg.cert_fingerprint.clone();
            st.selected = cfg.albums.clone();
            st.rules = cfg.album_rules.clone();
            st.cur = if st.rules.is_empty() { None } else { Some(0) };
            st.excluded = ui.excluded;
        }
        self.update_key_cue();
        self.update_pin_hint();
        self.update_http_warn();
        self.fill_rules();
        self.refill_grid(GRID_ALBUMS);
        let clean = self.form_key();
        self.st.borrow_mut().clean = clean;

        if let Some(e) = &ui.error {
            self.set_msg(&format!("config.json illisible : {e}"), Tone::Err);
        }
        if ui.has_key && !cfg.server_url.is_empty() {
            self.test(true);
        }
    }

    /// Configuration saisie dans la fenêtre (token vide = garder celui enregistré).
    fn form(&self) -> Config {
        let c = &self.c;
        let num = |h| get_text(h).trim().parse::<i64>().ok();
        let mut cfg = Config {
            server_url: get_text(c.e_url),
            api_key: get_text(c.e_key),
            insecure_skip_verify: self.on(c.t_insecure),
            interval_minutes: num(c.e_interval).filter(|&v| v >= 1).unwrap_or(60),
            style: self.sel_value(c.s_style, STYLES),
            image_size: self.sel_value(c.s_size, SIZES),
            multi_monitor: self.sel_value(c.s_monitor, MONITORS),
            offline_cache: num(c.e_offline).unwrap_or(0).clamp(0, 500),
            split_wide: self.on(c.t_split),
            landscape_only: self.on(c.t_landscape),
            change_on_start: self.on(c.t_start),
            change_on_unlock: self.on(c.t_unlock),
            active_from: get_text(c.e_from).trim().to_string(),
            active_to: get_text(c.e_to).trim().to_string(),
            pause_on_battery: self.on(c.t_battery),
            pause_on_metered: self.on(c.t_metered),
            pause_on_fullscreen: self.on(c.t_fullscreen),
            preload: self.on(c.t_preload),
            show_place: self.on(c.t_place),
            show_date: self.on(c.t_date),
            caption_position: self.sel_value(c.s_caption, CAPTIONS),
            desktop_link: self.on(c.t_link),
            lock_screen: self.on(c.t_lock),
            auto_update: self.on(c.t_update),
            ..Config::default()
        };
        let st = self.st.borrow();
        cfg.albums = st.selected.clone();
        cfg.album_rules = st.rules.iter().filter(|r| !r.albums.is_empty()).cloned().collect();
        cfg
    }

    fn form_key(&self) -> String {
        serde_json::to_string(&self.form()).unwrap_or_default()
    }

    fn dirty(&self) -> bool {
        let key = self.form_key();
        key != self.st.borrow().clean
    }

    /// Enregistre les réglages ; en cas d'erreur, l'explique et renvoie false.
    fn save(&self) -> bool {
        match self.settings.save(self.form()) {
            Err(e) => {
                self.message_box(&format!("Les réglages n'ont pas été enregistrés :\n\n{e}"), MB_ICONWARNING);
                false
            }
            Ok(saved) => {
                self.filling.set(true);
                set_text(self.c.e_key, "");
                self.filling.set(false);
                let fp_changed = {
                    let mut st = self.st.borrow_mut();
                    st.has_key = true;
                    let changed = st.cert_fp != saved.cert_fingerprint;
                    st.cert_fp = saved.cert_fingerprint.clone();
                    changed
                };
                self.update_key_cue();
                if fp_changed {
                    self.update_pin_hint();
                }
                let clean = self.form_key();
                self.st.borrow_mut().clean = clean;
                self.set_msg("Réglages enregistrés", Tone::Ok);
                true
            }
        }
    }

    /// Ferme la fenêtre, en proposant d'enregistrer les modifications.
    fn close(&self) {
        if self.dirty() {
            let r = self.message_box("Enregistrer les modifications avant de fermer ?", MB_YESNOCANCEL | MB_ICONQUESTION);
            if r == IDYES.0 {
                if !self.save() {
                    return;
                }
            } else if r != IDNO.0 {
                return;
            }
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    /// Vérifie la connexion et charge les albums en arrière-plan. quiet : à l'ouverture.
    fn test(&self, quiet: bool) {
        {
            let mut st = self.st.borrow_mut();
            if st.testing {
                return;
            }
            st.testing = true;
        }
        enable(self.c.b_test, false);
        enable(self.c.b_reload, false);
        if !quiet {
            self.set_test_msg("Connexion…", Tone::Muted);
        }
        let input = self.form();
        let s = self.settings.clone();
        let h = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.albums(input).map_err(|e| e.to_string())))
                .unwrap_or_else(|p| Err(format!("erreur interne : {}", panic_message(&*p))));
            finish(h, Done::Albums { quiet, res });
        });
    }

    /// Télécharge en arrière-plan les vignettes de couverture des albums affichés.
    fn load_covers(&self) {
        let input = self.form();
        let Ok(server) = normalize_server_url(&input.server_url) else {
            return;
        };
        let ids: Vec<String> = {
            let mut st = self.st.borrow_mut();
            st.cover_server = server;
            st.albums
                .iter()
                .map(|a| a.album_thumbnail_asset_id.clone())
                .filter(|s| !s.is_empty())
                .collect()
        };
        if ids.is_empty() {
            return;
        }
        let s = self.settings.clone();
        let h = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                s.album_covers(input, &ids, |_, _| finish(h, Done::Cover))
            }));
            if let Ok(Err(e)) = res {
                logf!("vignettes d'albums : {e}");
            }
        });
    }

    /// Change d'image en arrière-plan (comme le menu de l'icône).
    fn next(&self) {
        let Some(hooks) = self.settings.hooks().cloned() else { return };
        {
            let mut st = self.st.borrow_mut();
            if st.changing {
                return;
            }
            st.changing = true;
        }
        enable(self.c.b_next, false);
        self.set_msg("Changement en cours…", Tone::Muted);
        let h = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hooks.next()))
                .unwrap_or_else(|p| Err(format!("erreur interne : {}", panic_message(&*p))));
            finish(h, Done::Next(res));
        });
    }

    /// Résultats des tâches de fond destinés à cette fenêtre.
    fn take_done(&self) {
        let me = self.hwnd.0 as isize;
        let mine: Vec<Done> = {
            let mut q = lock(&DONE);
            let (mine, rest): (Vec<_>, Vec<_>) = q.drain(..).partition(|(h, _)| *h == me);
            *q = rest;
            mine.into_iter().map(|(_, d)| d).collect()
        };
        let mut covers = false;
        for d in mine {
            match d {
                Done::Albums { quiet, res } => {
                    self.st.borrow_mut().testing = false;
                    enable(self.c.b_test, true);
                    enable(self.c.b_reload, true);
                    match res {
                        Err(e) => self.set_test_msg(&e, Tone::Err),
                        Ok(albums) => {
                            let n = albums.len();
                            let go = {
                                let mut st = self.st.borrow_mut();
                                st.albums = albums;
                                !quiet && st.selected.is_empty() && st.page == PAGE_SERVER
                            };
                            LOADED_ALBUMS.store(n, Ordering::SeqCst);
                            self.set_test_msg(&format!("✓ Connecté — {n} albums"), Tone::Ok);
                            self.refill_grid(GRID_ALBUMS);
                            self.refill_grid(GRID_RULE);
                            self.load_covers();
                            if go {
                                self.show_page(PAGE_ALBUMS); // étape suivante : choisir les albums
                            }
                        }
                    }
                }
                Done::Cover => covers = true,
                Done::Next(res) => {
                    self.st.borrow_mut().changing = false;
                    enable(self.c.b_next, true);
                    match res {
                        Ok(m) => self.set_msg(&m, Tone::Ok),
                        Err(e) => self.set_msg(&e, Tone::Err),
                    }
                    self.refresh_status();
                }
            }
        }
        if covers {
            invalidate(self.c.grid_albums);
            invalidate(self.c.grid_rule);
        }
    }

    fn clear_excluded(&self) {
        if let Some(h) = self.settings.hooks() {
            if let Err(e) = h.clear_excluded() {
                self.set_msg(&e, Tone::Err);
                return;
            }
        }
        self.st.borrow_mut().excluded = 0;
        self.layout_content();
        self.set_msg("Photos réaffichées", Tone::Ok);
    }

    fn set_msg(&self, text: &str, tone: Tone) {
        self.st.borrow_mut().msg = (text.to_string(), tone);
        invalidate(self.hwnd);
    }

    fn set_test_msg(&self, text: &str, tone: Tone) {
        self.st.borrow_mut().test_msg = (text.to_string(), tone);
        self.layout_content();
    }

    fn refresh_status(&self) {
        let Some(h) = self.settings.hooks() else { return };
        let s = h.status();
        let changed = {
            let mut st = self.st.borrow_mut();
            let changed = st.last_status != s;
            st.last_status = s;
            changed
        };
        if changed {
            invalidate(self.hwnd);
        }
    }

    fn update_key_cue(&self) {
        let cue = if self.st.borrow().has_key {
            "•••••••• (enregistré — laisser vide pour le garder)"
        } else {
            "Colle ta clé API ici"
        };
        send_str(self.c.e_key, EM_SETCUEBANNER, 1, cue);
    }

    fn update_pin_hint(&self) {
        let fp = self.st.borrow().cert_fp.clone();
        let mut s = PIN_HINT.to_string();
        if fp.len() >= 16 {
            s += &format!(" Empreinte actuelle : {}…", &fp[..16]);
        }
        self.set_owner(self.c.t_insecure, |o| o.desc = s);
        self.layout_content();
    }

    fn update_http_warn(&self) {
        let on = plain_http_warning(&get_text(self.c.e_url));
        let changed = {
            let mut st = self.st.borrow_mut();
            let changed = st.http_warn != on;
            st.http_warn = on;
            changed
        };
        if changed {
            self.layout_content();
        }
    }

    /// Recrée les boutons de la liste des règles.
    fn fill_rules(&self) {
        let (n, old) = {
            let mut st = self.st.borrow_mut();
            let n = st.rules.len();
            st.cur = if n == 0 { None } else { st.cur.map(|c| c.min(n - 1)) };
            (n, std::mem::take(&mut st.rule_btns))
        };
        for b in old {
            self.owners.borrow_mut().remove(&(b.0 as isize));
            unsafe {
                let _ = DestroyWindow(b);
            }
        }
        let page = self.st.borrow().page;
        // Ordre de tabulation : la liste vient avant le bouton « Ajouter une règle ».
        let mut prev = self.pages[PAGE_SCHEDULE].last().copied().unwrap_or_default();
        let mut btns = vec![];
        for i in 0..n {
            let id = self.next_id.get() + 1;
            self.next_id.set(id);
            let b = create_owner(self.hinst, self.c.content, id, "");
            self.owners.borrow_mut().insert(
                b.0 as isize,
                Owner {
                    kind: Kind::Rule(i),
                    text: String::new(),
                    desc: String::new(),
                    icon: None,
                    on: false,
                    sel: 0,
                    bg: Bg::Card,
                },
            );
            unsafe {
                let _ = SetWindowPos(b, Some(prev), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE);
            }
            show_win(b, page == PAGE_RULES);
            prev = b;
            btns.push(b);
        }
        self.st.borrow_mut().rule_btns = btns;
        self.load_rule();
    }

    fn refresh_rule_buttons(&self) {
        for b in self.st.borrow().rule_btns.iter() {
            invalidate(*b);
        }
    }

    /// Affiche la règle sélectionnée dans l'éditeur.
    fn load_rule(&self) {
        let rule = {
            let st = self.st.borrow();
            st.cur.and_then(|i| st.rules.get(i).cloned())
        };
        let r = rule.unwrap_or_default();
        self.filling.set(true);
        self.set_sel(self.c.s_season, choice_index(SEASONS, &r.season));
        set_text(self.c.e_rfrom, &r.from);
        set_text(self.c.e_rto, &r.to);
        self.filling.set(false);
        self.grids[GRID_RULE].borrow_mut().scroll = 0;
        self.refill_grid(GRID_RULE);
        self.refresh_rule_buttons();
        self.layout_content();
    }
}

/// Le contrôle est actif (test).
#[cfg(test)]
fn is_enabled(h: HWND) -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h).as_bool() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{read_config, save_config};
    use crate::testutil::{temp_dir, FakeImmich, ALB1, ALB2, KEY, L1, L2};
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE};

    /// Traite les messages du thread jusqu'à ce que la condition soit vraie (au plus `secs` s).
    fn pump_until(secs: u64, cond: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            unsafe {
                let mut m = MSG::default();
                while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                    if !pre_translate(&m) {
                        let _ = TranslateMessage(&m);
                        DispatchMessageW(&m);
                    }
                }
            }
            if cond() {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn click(w: &Win, h: HWND) {
        let parent = if w.c.nav.contains(&h) || [w.c.b_next, w.c.b_close, w.c.b_save].contains(&h) {
            w.hwnd
        } else {
            w.c.content
        };
        let id = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(h) } as usize;
        send(parent, WM_COMMAND, id | ((BN_CLICKED as usize) << 16), h.0 as isize);
    }

    fn grid_index(w: &Win, i: usize, id: &str) -> usize {
        w.grids[i]
            .borrow()
            .rows
            .iter()
            .position(|r| r.id == id)
            .unwrap_or_else(|| panic!("album {id} absent de la grille {i}"))
    }

    fn sample(fake: &FakeImmich) -> Config {
        Config {
            server_url: fake.url.clone(),
            api_key: KEY.into(),
            albums: vec![ALB2.into()],
            interval_minutes: 25,
            style: "fit".into(),
            image_size: "preview".into(),
            multi_monitor: "different".into(),
            offline_cache: 12,
            split_wide: false,
            landscape_only: false,
            change_on_start: false,
            change_on_unlock: false,
            active_from: "22:00".into(),
            active_to: "07:30".into(),
            pause_on_battery: true,
            pause_on_metered: true,
            pause_on_fullscreen: true,
            preload: false,
            album_rules: vec![AlbumRule {
                albums: vec![ALB1.into()],
                season: "winter".into(),
                from: "08:00".into(),
                to: "12:00".into(),
            }],
            show_place: false,
            show_date: true,
            caption_position: "top-left".into(),
            desktop_link: false,
            lock_screen: true,
            auto_update: false,
            ..Config::default()
        }
    }

    /// Fenêtre remplie depuis la configuration, relue à l'identique ; grilles d'albums,
    /// vignettes, règles, interrupteurs, listes, enregistrement, erreurs, détection des
    /// modifications et changement de résolution.
    #[test]
    fn settings_window_round_trip() {
        let fake = FakeImmich::start();
        let cfg_path = temp_dir("settings_window").join("config.json");
        let cfg = sample(&fake);
        save_config(&cfg_path, &cfg).unwrap();
        let w = Win::create(Settings::new(&cfg_path)).expect("création de la fenêtre");
        let alerts = Rc::new(RefCell::new(Vec::<String>::new()));
        let answer = Rc::new(Cell::new(IDNO.0));
        {
            let (alerts, answer) = (alerts.clone(), answer.clone());
            *w.alert.borrow_mut() = Some(Box::new(move |t, _| {
                alerts.borrow_mut().push(t.to_string());
                answer.get()
            }));
        }
        assert!(is_open());

        // Chaque réglage est affiché puis relu tel quel (le token n'est jamais réaffiché).
        let mut want = cfg.clone();
        want.api_key.clear();
        assert_eq!(w.form(), want);
        assert!(!w.dirty(), "modifié dès l'ouverture");
        assert_eq!(get_text(w.c.e_key), "");

        // Le test de connexion lancé à l'ouverture remplit les grilles, puis les vignettes arrivent.
        assert!(pump_until(20, || loaded_albums() > 0), "albums non chargés");
        let n = w.st.borrow().albums.len();
        assert_eq!(w.grids[GRID_ALBUMS].borrow().rows.len(), n);
        let server = w.st.borrow().cover_server.clone();
        assert!(
            pump_until(20, || cached_cover(&server, L1).is_some() && cached_cover(&server, L2).is_some()),
            "vignettes non chargées"
        );

        // Cocher un album (clic ou espace) dans la grille.
        let i1 = grid_index(&w, GRID_ALBUMS, ALB1);
        w.grid_toggle(GRID_ALBUMS, i1);
        assert_eq!(w.st.borrow().selected, vec![ALB2.to_string(), ALB1.to_string()]);
        assert!(w.dirty());
        // Le filtre ne perd pas les albums cochés.
        set_text(w.c.e_filter, "zzz-aucun");
        assert_eq!(w.grids[GRID_ALBUMS].borrow().rows.len(), 0);
        set_text(w.c.e_filter, "finl");
        assert_eq!(w.grids[GRID_ALBUMS].borrow().rows.len(), 1);
        set_text(w.c.e_filter, "");
        assert_eq!(w.form().albums.len(), 2);

        // Navigation au clavier dans la grille.
        unsafe {
            let _ = SetFocus(Some(w.c.grid_albums));
        }
        w.grids[GRID_ALBUMS].borrow_mut().focus = 0;
        send(w.c.grid_albums, WM_KEYDOWN, VK_RIGHT, 0);
        assert_eq!(w.grids[GRID_ALBUMS].borrow().focus, 1.min(n - 1));

        // Règles : boutons de la liste, albums de la règle, ajout et suppression.
        assert_eq!(w.st.borrow().rule_btns.len(), 1);
        let r2 = grid_index(&w, GRID_RULE, ALB2);
        w.grid_toggle(GRID_RULE, r2);
        assert_eq!(w.form().album_rules[0].albums.len(), 2);
        set_text(w.c.e_rto, "13:00");
        assert_eq!(w.form().album_rules[0].to, "13:00");
        w.set_sel(w.c.s_season, choice_index(SEASONS, "summer"));
        w.select_changed(w.c.s_season);
        assert_eq!(w.form().album_rules[0].season, "summer");
        click(&w, w.c.b_add_rule);
        assert_eq!(w.st.borrow().rule_btns.len(), 2);
        assert_eq!(w.st.borrow().cur, Some(1));
        assert_eq!(w.form().album_rules.len(), 1, "règle sans album ignorée");
        let first = w.st.borrow().rule_btns[0];
        click(&w, first);
        assert_eq!(w.st.borrow().cur, Some(0));
        assert_eq!(get_text(w.c.e_rto), "13:00");
        click(&w, w.c.b_add_rule);
        assert_eq!(w.st.borrow().rules.len(), 3);
        click(&w, w.c.b_del_rule);
        assert_eq!(w.st.borrow().rules.len(), 2);
        assert_eq!(w.st.borrow().rule_btns.len(), 2);

        // Interrupteurs et listes.
        click(&w, w.c.t_lock);
        assert!(!w.form().lock_screen);
        click(&w, w.c.t_lock);
        assert!(w.form().lock_screen);
        w.set_sel(w.c.s_style, choice_index(STYLES, "span"));

        // Navigation entre les pages.
        click(&w, w.c.nav[PAGE_SCHEDULE]);
        assert_eq!(w.st.borrow().page, PAGE_SCHEDULE);
        let shown = |h: HWND| unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(h, windows::Win32::UI::WindowsAndMessaging::GWL_STYLE) } as u32 & WS_VISIBLE != 0;
        assert!(shown(w.c.e_from) && !shown(w.c.e_url));

        // Enregistrement : le token enregistré est conservé.
        set_text(w.c.e_interval, "7");
        assert!(w.save());
        assert!(!w.dirty());
        let saved = read_config(&cfg_path).unwrap();
        assert_eq!(saved.api_key, KEY);
        assert_eq!(saved.albums, vec![ALB2.to_string(), ALB1.to_string()]);
        assert_eq!(saved.interval_minutes, 7);
        assert_eq!(saved.style, "span");
        assert_eq!(saved.album_rules[0].albums.len(), 2);
        assert_eq!((saved.active_from.as_str(), saved.active_to.as_str()), ("22:00", "07:30"));

        // Erreurs : expliquées, rien n'est enregistré.
        set_text(w.c.e_url, "pas une url");
        assert!(!w.save());
        assert!(alerts.borrow()[0].contains("saisis de nouveau le token"), "{:?}", alerts.borrow());
        set_text(w.c.e_key, "autre-token");
        assert!(!w.save());
        assert!(alerts.borrow()[1].contains("server_url"), "{:?}", alerts.borrow());
        set_text(w.c.e_key, "");
        set_text(w.c.e_url, &fake.url);
        set_text(w.c.e_from, "25:00");
        assert!(!w.save());
        assert!(alerts.borrow()[2].contains("heure"), "{:?}", alerts.borrow());
        set_text(w.c.e_from, "22:00");
        assert_eq!(read_config(&cfg_path).unwrap().server_url, fake.url);
        // Avertissement http:// vers Internet.
        set_text(w.c.e_url, "http://immich.exemple.fr");
        assert!(w.st.borrow().http_warn);
        set_text(w.c.e_url, &fake.url);
        assert!(!w.st.borrow().http_warn && !w.dirty());

        // Test de connexion à la demande : bouton désactivé pendant le test.
        click(&w, w.c.nav[PAGE_SERVER]);
        click(&w, w.c.b_test);
        assert!(!is_enabled(w.c.b_test));
        assert!(pump_until(20, || is_enabled(w.c.b_test)), "test de connexion sans fin");
        assert_eq!(w.st.borrow().test_msg.1, Tone::Ok);

        // Changement de résolution (écran à 150 %) : polices et mise en page recalculées.
        let before = client_rect(w.c.b_save).bottom;
        let r = RECT {
            left: 0,
            top: 0,
            right: 1500,
            bottom: 1050,
        };
        send(w.hwnd, WM_DPICHANGED, (144 << 16) | 144, &r as *const RECT as isize);
        assert_eq!(w.dpi.get(), 144);
        assert!(client_rect(w.c.b_save).bottom > before, "bouton non agrandi");

        // Fermeture avec modifications : « Annuler » garde la fenêtre, « Non » la ferme.
        click(&w, w.c.t_lock);
        answer.set(2); // IDCANCEL
        send(w.hwnd, WM_CLOSE, 0, 0);
        assert!(is_open());
        answer.set(IDNO.0);
        send(w.hwnd, WM_CLOSE, 0, 0);
        assert!(pump_until(5, || !is_open()), "fenêtre non fermée");
        assert!(current().is_none());
        assert!(
            read_config(&cfg_path).unwrap().lock_screen,
            "modification enregistrée malgré « Non »"
        );
    }

    /// Appli lancée « cachée » (STARTF_USESHOWWINDOW + SW_HIDE, ce que fait la mise à jour depuis
    /// les versions Go) : Windows applique cet état au premier ShowWindow du processus. La fenêtre
    /// de réglages doit quand même s'afficher. Le test se relance lui-même dans un processus
    /// lancé ainsi, et y vérifie la fenêtre.
    #[test]
    fn settings_window_visible_after_hidden_start() {
        const CHILD: &str = "IW_HIDDEN_START_CHILD";
        const NAME: &str = "win::settings_window::tests::settings_window_visible_after_hidden_start";
        if std::env::var_os(CHILD).is_some() {
            let cfg_path = temp_dir("hidden_start").join("config.json");
            crate::config::write_default_config(&cfg_path).unwrap();
            let w = Win::create(Settings::new(&cfg_path)).expect("création de la fenêtre");
            w.show();
            pump_until(1, || false);
            let visible = unsafe { IsWindowVisible(w.hwnd) }.as_bool();
            *w.alert.borrow_mut() = Some(Box::new(|_, _| IDNO.0));
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
            assert!(visible, "fenêtre de réglages invisible après un lancement caché");
            return;
        }
        use windows::core::PWSTR;
        use windows::Win32::System::Threading::{
            CreateProcessW, GetExitCodeProcess, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
            STARTUPINFOW,
        };
        let exe = std::env::current_exe().unwrap();
        let mut line = wide(&format!("\"{}\" {NAME} --exact --test-threads=1 --nocapture", exe.display()));
        let mut env: Vec<u16> = vec![];
        for (k, v) in std::env::vars_os().chain([(CHILD.into(), "1".into())]) {
            env.extend(k.to_string_lossy().encode_utf16());
            env.push('=' as u16);
            env.extend(v.to_string_lossy().encode_utf16());
            env.push(0);
        }
        env.push(0);
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: 0, // SW_HIDE
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let code = unsafe {
            CreateProcessW(
                None,
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_UNICODE_ENVIRONMENT,
                Some(env.as_ptr() as *const _),
                None,
                &si,
                &mut pi,
            )
            .expect("relance du test");
            WaitForSingleObject(pi.hProcess, 120_000);
            let mut code = 1u32;
            let _ = GetExitCodeProcess(pi.hProcess, &mut code);
            let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
            let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
            code
        };
        assert_eq!(code, 0, "le processus lancé caché a échoué (voir sa sortie ci-dessus)");
    }

    /// Captures de chaque page en thème clair et sombre (IW_SETTINGS_SHOTS=<dossier>), pour
    /// vérifier la mise en page.
    #[test]
    fn settings_window_screenshots() {
        let Some(dir) = std::env::var_os("IW_SETTINGS_SHOTS") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = FakeImmich::start();
        for (theme, name) in [(1u8, "clair"), (2, "sombre")] {
            THEME_OVERRIDE.store(theme, Ordering::SeqCst);
            let cfg_path = temp_dir("settings_shots").join("config.json");
            let mut cfg = sample(&fake);
            cfg.album_rules[0].season = "summer".into();
            save_config(&cfg_path, &cfg).unwrap();
            let w = Win::create(Settings::new(&cfg_path)).unwrap();
            w.show();
            let server = normalize_server_url(&fake.url).unwrap();
            assert!(pump_until(20, || {
                loaded_albums() > 0 && cached_cover(&server, L1).is_some() && cached_cover(&server, L2).is_some()
            }));
            demo_albums(&w, &server);
            pump_until(1, || false);
            let files = [
                "serveur",
                "albums",
                "affichage",
                "planification",
                "saisons",
                "legende",
                "mises-a-jour",
            ];
            for (i, file) in files.iter().enumerate() {
                w.show_page(i);
                pump_until(1, || false);
                shot(w.hwnd, &dir.join(format!("{name}-{i}-{file}.png")));
            }
            *w.alert.borrow_mut() = Some(Box::new(|_, _| IDNO.0));
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
            pump_until(2, || !is_open());
        }
        THEME_OVERRIDE.store(0, Ordering::SeqCst);
    }

    /// Albums et vignettes colorées (dégradé, soleil, montagnes) pour des captures parlantes.
    fn demo_albums(w: &Win, server: &str) {
        let names = [
            ("Bassin", 2, (0x1f6f8b, 0xf4a259)),
            ("Finlande", 3, (0x0b1d51, 0x8fb8de)),
            ("Bretagne 2024", 214, (0x2a9d8f, 0xe9f5db)),
            ("Montagne", 87, (0x264653, 0xe76f51)),
            ("Japon", 532, (0x6d2e46, 0xf7b267)),
            ("Famille", 1290, (0x355070, 0xeaac8b)),
            ("Lumières du nord", 45, (0x03045e, 0x00b4d8)),
            ("Désert", 61, (0x9c6644, 0xffe8d6)),
        ];
        let mut albums = vec![];
        for (k, (name, count, (top, bottom))) in names.iter().enumerate() {
            let (id, thumb) = match k {
                0 => (ALB2.to_string(), L2.to_string()),
                1 => (ALB1.to_string(), L1.to_string()),
                _ => (
                    format!("{k:08}-0000-4000-8000-00000000000{k}"),
                    format!("{k:08}-1111-4000-8000-00000000000{k}"),
                ),
            };
            let side = 320u32;
            let mut px = Vec::with_capacity((side * side * 4) as usize);
            for y in 0..side {
                for x in 0..side {
                    let t = y as f32 / side as f32;
                    let mut c = mix(*top, *bottom, t);
                    let (dx, dy) = (x as f32 - side as f32 * 0.68, y as f32 - side as f32 * 0.36);
                    if dx * dx + dy * dy < (side as f32 * 0.11).powi(2) {
                        c = mix(c, 0xfff3c4, 0.85);
                    }
                    let ridge = side as f32 * (0.72 - 0.18 * ((x as f32 / side as f32) * 6.0 + k as f32).sin().abs());
                    if y as f32 > ridge {
                        c = mix(c, 0x10141c, 0.55);
                    }
                    px.extend_from_slice(&[(c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, (c >> 16) as u8, 255]);
                }
            }
            crate::settings::put_cover(server, &thumb, Cover { side, bgra: px });
            albums.push(AlbumInfo {
                id,
                album_name: name.to_string(),
                asset_count: *count,
                album_thumbnail_asset_id: thumb,
                ..Default::default()
            });
        }
        sort_albums(&mut albums);
        w.st.borrow_mut().albums = albums;
        w.refill_grid(GRID_ALBUMS);
        w.refill_grid(GRID_RULE);
    }

    /// Capture de la fenêtre (copie de l'écran à son emplacement).
    fn shot(hwnd: HWND, path: &std::path::Path) {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let (wd, ht) = (r.right - r.left, r.bottom - r.top);
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, wd, ht);
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let _ = BitBlt(mem, 0, 0, wd, ht, Some(screen), r.left, r.top, SRCCOPY);
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: wd,
                    biHeight: -ht,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut px = vec![0u8; (wd * ht * 4) as usize];
            GetDIBits(mem, bmp, 0, ht as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            for p in px.chunks_mut(4) {
                p.swap(0, 2);
                p[3] = 255;
            }
            image::RgbaImage::from_raw(wd as u32, ht as u32, px).unwrap().save(path).unwrap();
        }
    }
}
