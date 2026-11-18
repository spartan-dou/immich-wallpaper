//! Fenêtre de réglages native : contrôles standard de Windows dans une fenêtre à onglets.
//!
//! Elle vit sur le thread de l'interface (celui de l'icône de la barre système) : la boucle de
//! messages de l'appli lui passe d'abord chaque message (`pre_translate`) pour la navigation au
//! clavier des boîtes de dialogue. Le test de connexion et « Image suivante » tournent dans des
//! threads ; leur résultat revient par un message posté à la fenêtre. Aucun port n'est ouvert,
//! aucun navigateur n'est lancé.

use crate::config::{parse_clock, AlbumRule, Config};
use crate::immich::AlbumInfo;
use crate::logf;
use crate::settings::*;
use crate::util::{lock, panic_message};
use crate::win::platform::wide;
use crate::{is_portable, VERSION};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, DrawTextW, GetDC, GetMonitorInfoW, GetSysColor, GetSysColorBrush, InvalidateRect, MonitorFromWindow,
    ReleaseDC, SelectObject, SetBkColor, SetTextColor, COLOR_BTNFACE, COLOR_GRAYTEXT, COLOR_WINDOW, DT_CALCRECT, DT_EDITCONTROL,
    DT_NOPREFIX, DT_WORDBREAK, FONT_CHARSET, FONT_QUALITY, HBRUSH, HDC, HFONT, HGDIOBJ, LOGFONTW, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    SYS_COLOR_INDEX,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, EMF_CENTERED, ICC_DATE_CLASSES, ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES,
    ICC_UPDOWN_CLASS, INITCOMMONCONTROLSEX, LIST_VIEW_ITEM_STATE_FLAGS, LVCFMT_RIGHT, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW,
    LVIF_STATE, LVIF_TEXT, LVIS_STATEIMAGEMASK, LVITEMW, NMHDR, NMLISTVIEW, NMLVEMPTYMARKUP, TCIF_TEXT, TCITEMW,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, GetKeyState, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, GetClientRect, GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW,
    IsChild, IsDialogMessageW, IsIconic, KillTimer, LoadCursorW, MessageBoxW, PostMessageW, RegisterClassExW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, CW_USEDEFAULT, HICON, HMENU, IDC_ARROW, IDNO, IDYES,
    MB_ICONQUESTION, MB_ICONWARNING, MB_YESNOCANCEL, MESSAGEBOX_STYLE, MINMAXINFO, MSG, NONCLIENTMETRICSW, SM_CXICON, SM_CXSMICON,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW,
};

// ---------- valeurs Win32 (winuser.h, commctrl.h) ----------

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_EX_CLIENTEDGE: u32 = 0x0200;
const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;

const SS_LEFT: u32 = 0x0000;
const SS_LEFTNOWORDWRAP: u32 = 0x000C;
const SS_NOPREFIX: u32 = 0x0080;
const SS_CENTERIMAGE: u32 = 0x0200;
const SS_ENDELLIPSIS: u32 = 0x4000;
const BS_PUSHBUTTON: u32 = 0x0000;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const BS_MULTILINE: u32 = 0x2000;
const ES_PASSWORD: u32 = 0x0020;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_NUMBER: u32 = 0x2000;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const LVS_REPORT: u32 = 0x0001;
const LVS_SINGLESEL: u32 = 0x0004;
const LVS_SHOWSELALWAYS: u32 = 0x0008;
const LVS_NOCOLUMNHEADER: u32 = 0x4000;
const LVS_NOSORTHEADER: u32 = 0x8000;
const LVS_EX_CHECKBOXES: isize = 0x0004;
const LVS_EX_FULLROWSELECT: isize = 0x0020;
const LVS_EX_DOUBLEBUFFER: isize = 0x0001_0000;
const UDS_SETBUDDYINT: u32 = 0x0002;
const UDS_ALIGNRIGHT: u32 = 0x0004;
const UDS_ARROWKEYS: u32 = 0x0020;
const UDS_NOTHOUSANDS: u32 = 0x0080;
const UDS_HOTTRACK: u32 = 0x0100;
const DTS_SHOWNONE: u32 = 0x0002;
const DTS_TIMEFORMAT: u32 = 0x0009;

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_ACTIVATE: u32 = 0x0006;
const WM_SETREDRAW: u32 = 0x000B;
const WM_CLOSE: u32 = 0x0010;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_SETFONT: u32 = 0x0030;
const WM_NOTIFY: u32 = 0x004E;
const WM_SETICON: u32 = 0x0080;
const WM_NCDESTROY: u32 = 0x0082;
const WM_KEYDOWN: u32 = 0x0100;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const WM_CTLCOLORBTN: u32 = 0x0135;
const WM_CTLCOLORDLG: u32 = 0x0136;
const WM_CTLCOLORSTATIC: u32 = 0x0138;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_APP_DONE: u32 = 0x8000 + 20;
const DM_GETDEFID: u32 = 0x0400;
const DC_HASDEFID: isize = 0x534B;

const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const BN_CLICKED: u32 = 0;
const EN_CHANGE: u32 = 0x0300;
const EM_SETCUEBANNER: u32 = 0x1501;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const CBN_SELCHANGE: u32 = 1;
const LB_ADDSTRING: u32 = 0x0180;
const LB_INSERTSTRING: u32 = 0x0181;
const LB_DELETESTRING: u32 = 0x0182;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCURSEL: u32 = 0x0188;
const LBN_SELCHANGE: u32 = 1;
const LVM_DELETEALLITEMS: u32 = 0x1009;
const LVM_SETCOLUMNWIDTH: u32 = 0x101E;
const LVM_SETITEMSTATE: u32 = 0x102B;
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
const LVM_INSERTITEMW: u32 = 0x104D;
const LVM_INSERTCOLUMNW: u32 = 0x1061;
const LVM_SETITEMTEXTW: u32 = 0x1074;
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_SETCURSEL: u32 = 0x130C;
const TCM_ADJUSTRECT: u32 = 0x1328;
const TCM_INSERTITEMW: u32 = 0x133E;
const UDM_SETBUDDY: u32 = 0x0469;
const UDM_SETRANGE32: u32 = 0x046F;
const UDM_SETPOS32: u32 = 0x0471;
const DTM_GETSYSTEMTIME: u32 = 0x1001;
const DTM_SETSYSTEMTIME: u32 = 0x1002;
const DTM_SETFORMATW: u32 = 0x1032;
const GDT_VALID: isize = 0;
const GDT_NONE: usize = 1;

// Codes de notification (négatifs dans commctrl.h).
const LVN_ITEMCHANGED: u32 = 0xFFFF_FF9B; // LVN_FIRST - 1
const LVN_GETEMPTYMARKUP: u32 = 0xFFFF_FF45; // LVN_FIRST - 87
const TCN_SELCHANGE: u32 = 0xFFFF_FDD9; // TCN_FIRST - 1
const DTN_DATETIMECHANGE: u32 = 0xFFFF_FD09; // DTN_FIRST2 - 6

const VK_TAB: usize = 0x09;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;

const ID_SAVE: usize = 1; // IDOK : touche Entrée
const ID_CLOSE: usize = 2; // IDCANCEL : touche Échap
const TIMER_STATUS: usize = 1;

const CLASS_MAIN: PCWSTR = w!("ImmichWallpaperSettings");
const CLASS_PAGE: PCWSTR = w!("ImmichWallpaperSettingsPage");

// ---------- mise en page, en pixels à 100 % (96 ppp), mise à l'échelle de l'écran ----------

const DLG_W: i32 = 700; // zone cliente à l'ouverture
const DLG_H: i32 = 580;
const MIN_W: i32 = 600;
const MIN_H: i32 = 500;
const MARGIN: i32 = 12;
const PAGE_PAD: i32 = 16;
const LABEL_W: i32 = 176;
const ROW_H: i32 = 23;
const BTN_H: i32 = 26;
const CHECK_H: i32 = 19;
const CHECK_GLYPH: i32 = 20; // case à cocher et espace avant son texte
const CLOCK_W: i32 = 100;

const PAGE_SERVER: usize = 0;
const PAGE_ALBUMS: usize = 1;
const PAGE_NAMES: [&str; 7] = [
    "Serveur",
    "Albums",
    "Affichage",
    "Planification",
    "Saisons et heures",
    "Légende",
    "Mises à jour",
];

/// Couleurs de texte (COLORREF : 0x00BBGGRR) ; 0 = couleur normale.
const COLOR_OK: u32 = 0x003A_7A1F;
const COLOR_ERR: u32 = 0x0018_23B4;

// ---------- état partagé ----------

thread_local! {
    /// Fenêtre ouverte (une seule à la fois), sur le thread de l'interface.
    static CURRENT: RefCell<Option<Rc<Win>>> = const { RefCell::new(None) };
}

/// Fenêtre ouverte, lisible depuis les autres threads (vérification, tests).
static OPEN_HWND: AtomicIsize = AtomicIsize::new(0);
/// Albums chargés dans la fenêtre au dernier test de connexion réussi (vérification).
static LOADED_ALBUMS: AtomicUsize = AtomicUsize::new(0);

/// Résultats des tâches de fond, par fenêtre (handle), récupérés sur le thread de l'interface.
enum Done {
    Albums { quiet: bool, res: Result<Vec<AlbumInfo>, String> },
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
        unsafe {
            if IsIconic(w.hwnd).as_bool() {
                let _ = ShowWindow(w.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(w.hwnd);
        }
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
/// fenêtre (Tab, Entrée, Échap, Alt+lettre, Ctrl+Tab). true : le message a été traité.
pub fn pre_translate(msg: &MSG) -> bool {
    let Some(w) = current() else { return false };
    unsafe {
        if msg.message == WM_KEYDOWN
            && msg.wParam.0 == VK_TAB
            && GetKeyState(VK_CONTROL) < 0
            && (msg.hwnd == w.hwnd || IsChild(w.hwnd, msg.hwnd).as_bool())
        {
            let n = PAGE_NAMES.len();
            let d = if GetKeyState(VK_SHIFT) < 0 { n - 1 } else { 1 };
            let page = w.st.borrow().page;
            w.show_page((page + d) % n);
            let _ = SetFocus(Some(w.c.tab));
            return true;
        }
        IsDialogMessageW(w.hwnd, msg).as_bool()
    }
}

// ---------- petites fonctions Win32 ----------

fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}

fn send_ptr<T>(h: HWND, msg: u32, wp: usize, p: &T) -> isize {
    send(h, msg, wp, p as *const T as isize)
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

fn show_win(h: HWND, on: bool) {
    unsafe {
        let _ = ShowWindow(h, if on { SW_SHOW } else { SW_HIDE });
    }
}

fn enable(h: HWND, on: bool) {
    unsafe {
        let _ = EnableWindow(h, on);
    }
}

fn is_checked(h: HWND) -> bool {
    send(h, BM_GETCHECK, 0, 0) == 1
}

fn set_checked(h: HWND, on: bool) {
    send(h, BM_SETCHECK, on as usize, 0);
}

fn combo_sel(h: HWND) -> i32 {
    send(h, CB_GETCURSEL, 0, 0) as i32
}

fn set_combo(h: HWND, i: usize) {
    send(h, CB_SETCURSEL, i, 0);
}

fn client_rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    r
}

fn invalidate(h: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(h), None, true);
    }
}

fn sys_color(i: SYS_COLOR_INDEX) -> u32 {
    unsafe { GetSysColor(i) }
}

fn mul_div(v: i32, num: u32, den: u32) -> i32 {
    ((v as i64 * num as i64 + (den as i64) / 2) / den as i64) as i32
}

/// Police des boîtes de dialogue de Windows (Segoe UI 9 pt) à la résolution donnée.
fn message_font(dpi: u32) -> HFONT {
    let mut ncm = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    let mut lf = LOGFONTW {
        lfHeight: -mul_div(9, dpi, 72),
        lfWeight: 400,
        lfCharSet: FONT_CHARSET(1), // DEFAULT_CHARSET
        lfQuality: FONT_QUALITY(5), // CLEARTYPE_QUALITY
        ..Default::default()
    };
    for (d, s) in lf.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
        *d = s;
    }
    unsafe {
        if SystemParametersInfoForDpi(
            0x29, /* SPI_GETNONCLIENTMETRICS */
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut _),
            0,
            dpi,
        )
        .is_ok()
            && ncm.lfMessageFont.lfHeight != 0
        {
            lf = ncm.lfMessageFont;
        }
        CreateFontIndirectW(&lf)
    }
}

/// Hauteur du texte du contrôle une fois coupé à la largeur donnée.
fn text_height(h: HWND, font: HFONT, width: i32) -> i32 {
    let s = get_text(h);
    if s.is_empty() {
        return 0;
    }
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        let dc = GetDC(Some(h));
        if dc.is_invalid() {
            return 0;
        }
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut r = RECT {
            left: 0,
            top: 0,
            right: width.max(1),
            bottom: 0,
        };
        DrawTextW(dc, &mut buf, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX | DT_EDITCONTROL);
        SelectObject(dc, old);
        ReleaseDC(Some(h), dc);
        r.bottom - r.top
    }
}

/// Heure choisie, "" si la case « aucune heure » est décochée.
fn clock_value(h: HWND) -> String {
    let st = SYSTEMTIME::default();
    if send_ptr(h, DTM_GETSYSTEMTIME, 0, &st) != GDT_VALID {
        return String::new();
    }
    format!("{:02}:{:02}", st.wHour, st.wMinute)
}

/// Affiche l'heure v ; vide : case « aucune heure » décochée, avec `default` proposée quand on
/// la coche (plutôt que l'heure qu'il est).
fn set_clock(h: HWND, v: &str, default: &str) {
    let set = |m: u32| {
        let st = SYSTEMTIME {
            wYear: 2000,
            wMonth: 1,
            wDay: 1,
            wHour: (m / 60) as u16,
            wMinute: (m % 60) as u16,
            ..Default::default()
        };
        send_ptr(h, DTM_SETSYSTEMTIME, GDT_VALID as usize, &st);
    };
    match parse_clock(v) {
        Some(m) => set(m),
        None => {
            set(parse_clock(default).unwrap_or(0));
            send(h, DTM_SETSYSTEMTIME, GDT_NONE, 0);
        }
    }
}

/// Résultat d'une tâche de fond, remis à la fenêtre `hwnd` sur le thread de l'interface.
fn finish(hwnd: isize, d: Done) {
    lock(&DONE).push((hwnd, d));
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_DONE, WPARAM(0), LPARAM(0));
    }
}

// ---------- contrôles ----------

/// Handles des contrôles, créés une fois pour toutes.
#[derive(Default, Clone, Copy)]
struct Ctl {
    status: HWND,
    tab: HWND,
    pages: [HWND; 7],
    b_next: HWND,
    msg: HWND,
    b_save: HWND,
    b_close: HWND,
    // Serveur
    l_url: HWND,
    e_url: HWND,
    w_http: HWND,
    l_key: HWND,
    e_key: HWND,
    h_key: HWND,
    c_insecure: HWND,
    h_pin: HWND,
    b_test: HWND,
    test_msg: HWND,
    // Albums
    e_filter: HWND,
    albums_msg: HWND,
    b_reload: HWND,
    lv_albums: HWND,
    album_count: HWND,
    // Affichage
    l_interval: HWND,
    e_interval: HWND,
    ud_interval: HWND,
    s_minutes: HWND,
    l_style: HWND,
    cb_style: HWND,
    l_size: HWND,
    cb_size: HWND,
    l_monitor: HWND,
    cb_monitor: HWND,
    l_offline: HWND,
    e_offline: HWND,
    ud_offline: HWND,
    h_offline: HWND,
    c_split: HWND,
    c_landscape: HWND,
    c_start: HWND,
    c_unlock: HWND,
    excluded_info: HWND,
    b_show_excluded: HWND,
    // Planification
    l_hours: HWND,
    dt_from: HWND,
    s_to: HWND,
    dt_to: HWND,
    h_hours: HWND,
    c_battery: HWND,
    c_metered: HWND,
    c_fullscreen: HWND,
    h_pauses: HWND,
    c_preload: HWND,
    h_preload: HWND,
    // Saisons et heures
    h_rules: HWND,
    lb_rules: HWND,
    b_add_rule: HWND,
    b_del_rule: HWND,
    l_season: HWND,
    cb_season: HWND,
    l_rhours: HWND,
    dt_rfrom: HWND,
    s_rto: HWND,
    dt_rto: HWND,
    l_ralbums: HWND,
    lv_rule: HWND,
    // Légende et écran de verrouillage
    c_place: HWND,
    c_date: HWND,
    l_caption: HWND,
    cb_caption: HWND,
    c_link: HWND,
    h_link: HWND,
    c_lock: HWND,
    h_lock: HWND,
    // Mises à jour
    c_update: HWND,
    h_update_portable: HWND,
    h_update: HWND,
    s_version: HWND,
}

/// Création des contrôles : police, identifiants, ordre de tabulation.
struct Builder {
    hinst: HINSTANCE,
    font: HFONT,
    dpi: u32,
    next_id: usize,
    ctls: Vec<HWND>,
    children: Vec<(HWND, Vec<HWND>)>,
    colors: HashMap<isize, u32>,
}

impl Builder {
    fn add_id(&mut self, parent: HWND, class: &str, text: &str, style: u32, ex: u32, id: usize) -> HWND {
        let (c, t) = (wide(class), wide(text));
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(ex),
                PCWSTR(c.as_ptr()),
                PCWSTR(t.as_ptr()),
                WINDOW_STYLE(style | WS_CHILD | WS_VISIBLE),
                0,
                0,
                0,
                0,
                Some(parent),
                Some(HMENU(id as *mut _)),
                Some(self.hinst),
                None,
            )
        };
        let h = h.unwrap_or_else(|e| panic!("création du contrôle {class} : {e}"));
        send(h, WM_SETFONT, self.font.0 as usize, 0);
        self.ctls.push(h);
        match self.children.iter_mut().find(|(p, _)| *p == parent) {
            Some((_, list)) => list.push(h),
            None => self.children.push((parent, vec![h])),
        }
        h
    }

    fn add(&mut self, parent: HWND, class: &str, text: &str, style: u32, ex: u32) -> HWND {
        self.next_id += 1;
        let id = self.next_id;
        self.add_id(parent, class, text, style, ex, id)
    }

    fn label(&mut self, p: HWND, text: &str) -> HWND {
        self.add(p, "STATIC", text, SS_LEFTNOWORDWRAP | SS_CENTERIMAGE, 0)
    }

    fn line(&mut self, p: HWND) -> HWND {
        self.add(
            p,
            "STATIC",
            "",
            SS_LEFTNOWORDWRAP | SS_ENDELLIPSIS | SS_NOPREFIX | SS_CENTERIMAGE,
            0,
        )
    }

    fn text(&mut self, p: HWND, text: &str) -> HWND {
        self.add(p, "STATIC", text, SS_LEFT | SS_NOPREFIX, 0)
    }

    fn hint(&mut self, p: HWND, text: &str) -> HWND {
        let h = self.text(p, text);
        self.colors.insert(h.0 as isize, sys_color(COLOR_GRAYTEXT));
        h
    }

    fn check(&mut self, p: HWND, text: &str) -> HWND {
        self.add(p, "BUTTON", text, BS_AUTOCHECKBOX | BS_MULTILINE | WS_TABSTOP, 0)
    }

    fn button(&mut self, p: HWND, text: &str) -> HWND {
        self.add(p, "BUTTON", text, BS_PUSHBUTTON | WS_TABSTOP, 0)
    }

    fn edit(&mut self, p: HWND, style: u32) -> HWND {
        self.add(p, "EDIT", "", ES_AUTOHSCROLL | WS_TABSTOP | style, WS_EX_CLIENTEDGE)
    }

    fn combo(&mut self, p: HWND, list: &[Choice]) -> HWND {
        let h = self.add(p, "COMBOBOX", "", CBS_DROPDOWNLIST | WS_TABSTOP | WS_VSCROLL, 0);
        for c in list {
            send_str(h, CB_ADDSTRING, 0, c.1);
        }
        set_combo(h, 0);
        h
    }

    /// Zone de saisie numérique avec ses flèches.
    fn number(&mut self, p: HWND, lo: i32, hi: i32) -> (HWND, HWND) {
        let e = self.edit(p, ES_NUMBER);
        let ud = self.add(
            p,
            "msctls_updown32",
            "",
            UDS_SETBUDDYINT | UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS | UDS_HOTTRACK,
            0,
        );
        send(ud, UDM_SETBUDDY, e.0 as usize, 0);
        send(ud, UDM_SETRANGE32, lo as usize, hi as isize);
        (e, ud)
    }

    /// Heure « HH:mm » avec une case « aucune heure ».
    fn clock(&mut self, p: HWND) -> HWND {
        let h = self.add(p, "SysDateTimePick32", "", DTS_TIMEFORMAT | DTS_SHOWNONE | WS_TABSTOP, 0);
        send_str(h, DTM_SETFORMATW, 0, "HH':'mm");
        h
    }

    /// Liste d'albums à cocher.
    fn list(&mut self, p: HWND, columns: &[&str]) -> HWND {
        let mut style = LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SINGLESEL | LVS_NOSORTHEADER | WS_TABSTOP;
        if columns.len() == 1 {
            style |= LVS_NOCOLUMNHEADER;
        }
        let h = self.add(p, "SysListView32", "", style, WS_EX_CLIENTEDGE);
        send(
            h,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER,
        );
        unsafe {
            let _ = SetWindowTheme(h, w!("Explorer"), PCWSTR::null());
        }
        for (i, name) in columns.iter().enumerate() {
            let mut t = wide(name);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if i > 0 { LVCFMT_RIGHT } else { Default::default() },
                cx: mul_div(100, self.dpi, 96),
                pszText: PWSTR(t.as_mut_ptr()),
                ..Default::default()
            };
            send_ptr(h, LVM_INSERTCOLUMNW, i, &col);
        }
        h
    }
}

fn build(b: &mut Builder, main: HWND) -> Ctl {
    let mut c = Ctl {
        status: b.line(main),
        ..Default::default()
    };
    b.colors.insert(c.status.0 as isize, sys_color(COLOR_GRAYTEXT));
    let class = unsafe { String::from_utf16_lossy(CLASS_PAGE.as_wide()) };
    for p in c.pages.iter_mut() {
        *p = b.add(main, &class, "", WS_CLIPCHILDREN | WS_CLIPSIBLINGS, WS_EX_CONTROLPARENT);
    }

    // Serveur
    let p = c.pages[0];
    c.l_url = b.label(p, "&URL du serveur");
    c.e_url = b.edit(p, 0);
    send_str(c.e_url, EM_SETCUEBANNER, 0, "https://immich.exemple.fr");
    c.w_http = b.text(
        p,
        "Connexion non chiffrée (http://) vers un serveur hors réseau local : le token et les photos circulent en clair. \
         Préfère https://.",
    );
    b.colors.insert(c.w_http.0 as isize, COLOR_ERR);
    c.l_key = b.label(p, "T&oken (clé API)");
    c.e_key = b.edit(p, ES_PASSWORD);
    c.h_key = b.hint(
        p,
        "Immich → Paramètres du compte → Clés API. Permissions : album.read, asset.read, asset.view, asset.download \
         (et asset.update pour « Ajouter aux favoris »).",
    );
    c.c_insecure = b.check(p, "Accepter un certificat auto-signé");
    c.h_pin = b.hint(p, "");
    c.b_test = b.button(p, "&Tester et charger les albums");
    c.test_msg = b.text(p, "");

    // Albums
    let p = c.pages[1];
    c.e_filter = b.edit(p, 0);
    send_str(c.e_filter, EM_SETCUEBANNER, 0, "Filtrer…");
    c.albums_msg = b.line(p);
    c.b_reload = b.button(p, "&Recharger la liste");
    c.lv_albums = b.list(p, &["Album", "Éléments"]);
    c.album_count = b.line(p);

    // Affichage
    let p = c.pages[2];
    c.l_interval = b.label(p, "Changer d'image toutes les");
    (c.e_interval, c.ud_interval) = b.number(p, 1, 99999);
    c.s_minutes = b.label(p, "minutes");
    c.l_style = b.label(p, "Ajustement");
    c.cb_style = b.combo(p, STYLES);
    c.l_size = b.label(p, "Qualité");
    c.cb_size = b.combo(p, SIZES);
    c.l_monitor = b.label(p, "Plusieurs écrans");
    c.cb_monitor = b.combo(p, MONITORS);
    c.l_offline = b.label(p, "Photos gardées hors ligne");
    (c.e_offline, c.ud_offline) = b.number(p, 0, 500);
    c.h_offline = b.hint(
        p,
        "Si Immich est injoignable (déplacement, VPN coupé…), la rotation continue avec les dernières photos \
         téléchargées. 0 = désactivé.",
    );
    c.c_split = b.check(p, "Écran très large (32:9, « deux écrans en un ») : deux photos côte à côte");
    c.c_landscape = b.check(p, "Uniquement les photos en paysage");
    c.c_start = b.check(p, "Changer d'image au démarrage");
    c.c_unlock = b.check(p, "Changer d'image à chaque déverrouillage du PC");
    c.excluded_info = b.hint(p, "");
    c.b_show_excluded = b.button(p, "Les réafficher");

    // Planification
    let p = c.pages[3];
    c.l_hours = b.label(p, "Changer d'image de");
    c.dt_from = b.clock(p);
    c.s_to = b.label(p, "à");
    c.dt_to = b.clock(p);
    c.h_hours = b.hint(
        p,
        "Décoche les deux heures pour changer toute la journée. Une plage comme 22:00 → 07:00 passe par minuit.",
    );
    c.c_battery = b.check(p, "Ne pas changer sur batterie");
    c.c_metered = b.check(
        p,
        "Ne pas changer sur une connexion limitée (partage de connexion, forfait de données)",
    );
    c.c_fullscreen = b.check(
        p,
        "Ne pas changer pendant une application en plein écran (jeu, vidéo, présentation)",
    );
    c.h_pauses = b.hint(
        p,
        "Ces réglages ne concernent que les changements automatiques : « Image suivante » fonctionne toujours. \
         Le changement se fait dès que la condition disparaît.",
    );
    c.c_preload = b.check(p, "Télécharger la photo suivante à l'avance");
    c.h_preload = b.hint(
        p,
        "Le changement suivant est alors immédiat et ne dépend plus du réseau (utile au déverrouillage du PC).",
    );

    // Saisons et heures
    let p = c.pages[4];
    c.h_rules = b.hint(
        p,
        "Quand une règle s'applique, ses albums remplacent les albums habituels (onglet Albums). Plusieurs règles \
         en même temps : leurs albums sont réunis.",
    );
    c.lb_rules = b.add(
        p,
        "LISTBOX",
        "",
        LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | WS_VSCROLL | WS_TABSTOP,
        WS_EX_CLIENTEDGE,
    );
    c.b_add_rule = b.button(p, "&Ajouter une règle");
    c.b_del_rule = b.button(p, "Supprimer");
    c.l_season = b.label(p, "Saison");
    c.cb_season = b.combo(p, SEASONS);
    c.l_rhours = b.label(p, "De");
    c.dt_rfrom = b.clock(p);
    c.s_rto = b.label(p, "à");
    c.dt_rto = b.clock(p);
    c.l_ralbums = b.label(p, "Albums de cette règle :");
    c.lv_rule = b.list(p, &["Album"]);

    // Légende et écran de verrouillage
    let p = c.pages[5];
    c.c_place = b.check(p, "Afficher le lieu sur l'image (ville, région, pays — photos géolocalisées)");
    c.c_date = b.check(p, "Afficher le mois et l'année");
    c.l_caption = b.label(p, "Position du texte");
    c.cb_caption = b.combo(p, CAPTIONS);
    c.c_link = b.check(p, "Icône « Voir cette photo dans Immich » sur le bureau");
    c.h_link = b.hint(
        p,
        "Le fond d'écran lui-même n'est pas cliquable sous Windows : cette icône (ou un clic gauche sur l'icône près \
         de l'horloge) ouvre la photo affichée.",
    );
    c.c_lock = b.check(p, "Appliquer aussi à l'écran de verrouillage");
    c.h_lock = b.hint(
        p,
        "Dans Windows : Paramètres → Personnalisation → Écran de verrouillage → « Image » (et non « Windows à la une »).",
    );

    // Mises à jour
    let p = c.pages[6];
    c.c_update = b.check(p, "Installer automatiquement les nouvelles versions publiées sur GitHub");
    c.h_update_portable = b.hint(
        p,
        "Version portable : une nouvelle version est seulement annoncée dans le menu de l'icône (« Installer la \
         version… ») ; rien n'est téléchargé ni remplacé sans ton accord. Décoché, l'appli ne contacte plus GitHub \
         d'elle-même.",
    );
    c.h_update = b.hint(
        p,
        "Vérification au démarrage puis une fois par jour. Téléchargement depuis les releases GitHub du projet \
         uniquement ; l'empreinte SHA-256 détecte un fichier corrompu (les releases ne sont pas signées).",
    );
    c.s_version = b.hint(p, "");

    // Barre du bas
    c.b_next = b.button(main, "Image &suivante");
    c.msg = b.text(main, "");
    c.b_save = b.add_id(main, "BUTTON", "&Enregistrer", BS_DEFPUSHBUTTON | WS_TABSTOP, 0, ID_SAVE);
    c.b_close = b.add_id(main, "BUTTON", "Fermer", BS_PUSHBUTTON | WS_TABSTOP, 0, ID_CLOSE);

    // Onglets en dernier : sous les pages qui les recouvrent, et derniers dans l'ordre de tabulation.
    c.tab = b.add(main, "SysTabControl32", "", WS_CLIPSIBLINGS | WS_TABSTOP, 0);
    for (i, name) in PAGE_NAMES.iter().enumerate() {
        let mut t = wide(name);
        let it = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(t.as_mut_ptr()),
            ..Default::default()
        };
        send_ptr(c.tab, TCM_INSERTITEMW, i, &it);
    }
    // Ordre de tabulation (et d'empilement) = ordre de création.
    for (_, list) in &b.children {
        for pair in list.windows(2) {
            unsafe {
                let _ = SetWindowPos(pair[1], Some(pair[0]), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE);
            }
        }
    }
    c
}

// ---------- fenêtre ----------

#[derive(Default)]
struct State {
    albums: Vec<AlbumInfo>, // albums chargés depuis Immich (triés)
    selected: Vec<String>,  // albums habituels cochés, dans l'ordre
    rows: Vec<String>,      // identifiants affichés dans la liste des albums
    rules: Vec<AlbumRule>,
    cur: Option<usize>,     // règle sélectionnée
    rule_rows: Vec<String>, // identifiants affichés dans la liste des albums de la règle
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
}

/// Réponse d'une boîte de message (remplaçable par les tests).
type Alert = Box<dyn FnMut(&str, MESSAGEBOX_STYLE) -> i32>;

struct Win {
    settings: Arc<Settings>,
    hwnd: HWND,
    c: Ctl,
    ctls: Vec<HWND>,
    font: Cell<HFONT>,
    dpi: Cell<u32>,
    icons: Cell<(HICON, HICON)>,
    ready: Cell<bool>,
    /// Remplissage d'une liste en cours : ses notifications sont ignorées.
    filling: Cell<bool>,
    portable: bool,
    st: RefCell<State>,
    colors: RefCell<HashMap<isize, u32>>,
    alert: RefCell<Option<Alert>>,
}

fn register_classes(hinst: HINSTANCE) -> Result<(), String> {
    static ONCE: Once = Once::new();
    static ERR: Mutex<Option<String>> = Mutex::new(None);
    ONCE.call_once(|| unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for (class, bg) in [(CLASS_MAIN, COLOR_BTNFACE), (CLASS_PAGE, COLOR_WINDOW)] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                hCursor: cursor,
                hbrBackground: HBRUSH((bg.0 + 1) as isize as *mut _),
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
        if w.hwnd == hwnd || w.c.pages.contains(&hwnd) {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.handle(hwnd, msg, wp, lp))) {
                Ok(Some(r)) => return r,
                Ok(None) => {}
                Err(p) => logf!("panic (fenêtre de réglages) : {}", panic_message(&*p)),
            }
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Place les contrôles d'une page de haut en bas.
struct Flow<'a> {
    w: &'a Win,
    x: i32,
    y: i32,
    cw: i32, // en pixels
}

impl Flow<'_> {
    fn px(&self, v: i32) -> i32 {
        self.w.px(v)
    }

    fn gap(&mut self, d: i32) {
        self.y += self.px(d);
    }

    /// Libellé dans la colonne de gauche, contrôle à droite (largeur wd, 0 = jusqu'au bord).
    fn row(&mut self, label: HWND, ctl: HWND, wd: i32) {
        let cx = self.x + self.px(LABEL_W);
        let mut cw = self.x + self.cw - cx;
        if wd > 0 {
            cw = cw.min(self.px(wd));
        }
        move_win(label, self.x, self.y, self.px(LABEL_W - 8), self.px(ROW_H));
        move_win(ctl, cx, self.y, cw, self.px(ROW_H));
        self.y += self.px(ROW_H);
    }

    /// Comme row ; la hauteur donnée à une liste déroulante est celle de sa liste ouverte.
    fn combo(&mut self, label: HWND, ctl: HWND, wd: i32) {
        let cx = self.x + self.px(LABEL_W);
        move_win(label, self.x, self.y, self.px(LABEL_W - 8), self.px(ROW_H));
        move_win(ctl, cx, self.y, self.px(wd).min(self.x + self.cw - cx), self.px(240));
        self.y += self.px(ROW_H);
    }

    /// Zone numérique, ses flèches et un texte après.
    fn number(&mut self, label: HWND, edit: HWND, updown: HWND, suffix: Option<HWND>) {
        self.row(label, edit, 84);
        send(updown, UDM_SETBUDDY, edit.0 as usize, 0); // replace les flèches contre la zone déplacée
        if let Some(s) = suffix {
            move_win(
                s,
                self.x + self.px(LABEL_W + 84 + 8),
                self.y - self.px(ROW_H),
                self.px(120),
                self.px(ROW_H),
            );
        }
    }

    /// Case à cocher décalée de dx (pixels à 100 %), sur une ou plusieurs lignes.
    fn check(&mut self, h: HWND, dx: i32) {
        let wd = self.cw - self.px(dx);
        let ht = self
            .px(CHECK_H)
            .max(text_height(h, self.w.font.get(), wd - self.px(CHECK_GLYPH)) + self.px(3));
        move_win(h, self.x + self.px(dx), self.y, wd, ht);
        self.y += ht;
    }

    /// Texte coupé à la largeur disponible, décalé de dx.
    fn text(&mut self, h: HWND, dx: i32) {
        let wd = self.cw - self.px(dx);
        let ht = text_height(h, self.w.font.get(), wd);
        move_win(h, self.x + self.px(dx), self.y, wd, ht);
        self.y += ht;
    }
}

impl Win {
    fn create(settings: Arc<Settings>) -> Result<Rc<Win>, String> {
        unsafe {
            let icc = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_STANDARD_CLASSES | ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES | ICC_UPDOWN_CLASS | ICC_DATE_CLASSES,
            };
            let _ = InitCommonControlsEx(&icc);
        }
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
        let font = message_font(dpi);
        let mut b = Builder {
            hinst,
            font,
            dpi,
            next_id: 100,
            ctls: vec![],
            children: vec![],
            colors: HashMap::new(),
        };
        let c = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut b, hwnd))) {
            Ok(c) => c,
            Err(p) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
                return Err(panic_message(&*p));
            }
        };
        let w = Rc::new(Win {
            settings,
            hwnd,
            c,
            ctls: b.ctls,
            font: Cell::new(font),
            dpi: Cell::new(dpi),
            icons: Cell::new((HICON::default(), HICON::default())),
            ready: Cell::new(false),
            filling: Cell::new(false),
            portable: is_portable(),
            st: RefCell::new(State {
                cur: None,
                ..Default::default()
            }),
            colors: RefCell::new(b.colors),
            alert: RefCell::new(None),
        });
        CURRENT.with(|cur| *cur.borrow_mut() = Some(w.clone()));
        OPEN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        w.set_icons();
        w.show_page(PAGE_SERVER);
        w.st.borrow_mut().focus = w.c.e_url.0 as isize;
        w.load();
        w.ready.set(true);
        w.place_default();
        w.layout();
        Ok(w)
    }

    fn px(&self, v: i32) -> i32 {
        mul_div(v, self.dpi.get(), 96)
    }

    fn set_icons(&self) {
        unsafe {
            let big = crate::win::app::load_icon(GetSystemMetrics(SM_CXICON));
            let small = crate::win::app::load_icon(GetSystemMetrics(SM_CXSMICON));
            if let Some(i) = big {
                send(self.hwnd, WM_SETICON, 1 /* ICON_BIG */, i.0 as isize);
            }
            if let Some(i) = small {
                send(self.hwnd, WM_SETICON, 0 /* ICON_SMALL */, i.0 as isize);
            }
            self.icons.set((big.unwrap_or_default(), small.unwrap_or_default()));
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
            let _ = SetForegroundWindow(self.hwnd);
            SetTimer(Some(self.hwnd), TIMER_STATUS, 2000, None);
        }
        self.refresh_status();
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
            let _ = DeleteObject(HGDIOBJ(self.font.get().0));
            let (a, b) = self.icons.get();
            for i in [a, b] {
                if !i.is_invalid() {
                    let _ = DestroyIcon(i);
                }
            }
        }
        lock(&DONE).retain(|(h, _)| *h != self.hwnd.0 as isize);
        OPEN_HWND.store(0, Ordering::SeqCst);
        CURRENT.with(|c| *c.borrow_mut() = None);
    }

    // ---------- mise en page ----------

    fn layout(&self) {
        if !self.ready.get() {
            return;
        }
        let c = &self.c;
        let p = |v| self.px(v);
        let rc = client_rect(self.hwnd);
        let (width, height) = (rc.right, rc.bottom);
        let m = p(MARGIN);
        move_win(c.status, m, p(8), width - 2 * m, p(22));

        let by = height - m - p(BTN_H);
        let (close_w, save_w, next_w) = (p(96), p(112), p(128));
        move_win(c.b_close, width - m - close_w, by, close_w, p(BTN_H));
        move_win(c.b_save, width - m - close_w - p(8) - save_w, by, save_w, p(BTN_H));
        move_win(c.b_next, m, by, next_w, p(BTN_H));
        self.place_msg();

        let ty = p(34);
        let (tw, th) = (width - 2 * m, by - p(12) - ty);
        move_win(c.tab, m, ty, tw, th);
        let mut r = RECT {
            left: 0,
            top: 0,
            right: tw,
            bottom: th,
        };
        send(c.tab, TCM_ADJUSTRECT, 0, &mut r as *mut RECT as isize);
        let (pw, ph) = (r.right - r.left, r.bottom - r.top);
        for pg in c.pages {
            move_win(pg, m + r.left, ty + r.top, pw, ph);
        }
        let pad = p(PAGE_PAD);
        let bottom = ph - pad;
        let flow = || Flow {
            w: self,
            x: pad,
            y: pad - p(2),
            cw: pw - 2 * pad,
        };
        let (http_warn, excluded) = {
            let st = self.st.borrow();
            (st.http_warn, st.excluded)
        };

        // Serveur
        let mut f = flow();
        f.row(c.l_url, c.e_url, 0);
        show_win(c.w_http, http_warn);
        if http_warn {
            f.gap(4);
            f.text(c.w_http, LABEL_W);
        }
        f.gap(10);
        f.row(c.l_key, c.e_key, 0);
        f.gap(4);
        f.text(c.h_key, LABEL_W);
        f.gap(12);
        f.check(c.c_insecure, LABEL_W);
        f.gap(2);
        f.text(c.h_pin, LABEL_W + CHECK_GLYPH);
        f.gap(16);
        move_win(c.b_test, f.x + p(LABEL_W), f.y, p(230).min(f.cw - p(LABEL_W)), p(BTN_H));
        f.y += p(BTN_H);
        f.gap(8);
        f.text(c.test_msg, LABEL_W);

        // Albums
        let f = flow();
        let fw = p(240).min(f.cw / 2);
        let bw = p(150);
        move_win(c.e_filter, f.x, f.y + (p(BTN_H) - p(ROW_H)) / 2, fw, p(ROW_H));
        move_win(c.b_reload, f.x + f.cw - bw, f.y, bw, p(BTN_H));
        move_win(c.albums_msg, f.x + fw + p(12), f.y, f.cw - fw - bw - p(24), p(BTN_H));
        let y = f.y + p(BTN_H) + p(8);
        let count_h = p(22);
        let lv_h = (bottom - y - count_h - p(4)).max(p(60));
        move_win(c.lv_albums, f.x, y, f.cw, lv_h);
        self.size_columns(c.lv_albums, p(100));
        move_win(c.album_count, f.x, y + lv_h + p(4), f.cw, count_h);

        // Affichage
        let mut f = flow();
        f.number(c.l_interval, c.e_interval, c.ud_interval, Some(c.s_minutes));
        f.gap(10);
        f.combo(c.l_style, c.cb_style, 280);
        f.gap(8);
        f.combo(c.l_size, c.cb_size, 280);
        f.gap(8);
        f.combo(c.l_monitor, c.cb_monitor, 280);
        f.gap(10);
        f.number(c.l_offline, c.e_offline, c.ud_offline, None);
        f.gap(4);
        f.text(c.h_offline, LABEL_W);
        f.gap(14);
        for h in [c.c_split, c.c_landscape, c.c_start, c.c_unlock] {
            f.check(h, 0);
            f.gap(4);
        }
        show_win(c.excluded_info, excluded > 0);
        show_win(c.b_show_excluded, excluded > 0);
        if excluded > 0 {
            f.gap(10);
            let ebw = p(130);
            let ew = f.cw - ebw - p(12);
            let eh = text_height(c.excluded_info, self.font.get(), ew);
            move_win(c.excluded_info, f.x, f.y + (p(BTN_H) - eh).max(0) / 2, ew, eh);
            move_win(c.b_show_excluded, f.x + f.cw - ebw, f.y, ebw, p(BTN_H));
        }

        // Planification
        let mut f = flow();
        let cw = p(CLOCK_W);
        let lx = f.x + p(LABEL_W);
        move_win(c.l_hours, f.x, f.y, p(LABEL_W - 8), p(ROW_H));
        move_win(c.dt_from, lx, f.y, cw, p(ROW_H));
        move_win(c.s_to, lx + cw + p(10), f.y, p(20), p(ROW_H));
        move_win(c.dt_to, lx + cw + p(34), f.y, cw, p(ROW_H));
        f.y += p(ROW_H);
        f.gap(4);
        f.text(c.h_hours, LABEL_W);
        f.gap(16);
        for h in [c.c_battery, c.c_metered, c.c_fullscreen] {
            f.check(h, 0);
            f.gap(4);
        }
        f.text(c.h_pauses, CHECK_GLYPH);
        f.gap(16);
        f.check(c.c_preload, 0);
        f.gap(2);
        f.text(c.h_preload, CHECK_GLYPH);

        // Saisons et heures
        let mut f = flow();
        f.text(c.h_rules, 0);
        f.gap(12);
        let top = f.y;
        let lw = p(230).min(f.cw * 2 / 5);
        let lb_h = (bottom - top - p(BTN_H) - p(8)).max(p(60));
        move_win(c.lb_rules, f.x, top, lw, lb_h);
        let half = (lw - p(8)) / 2;
        move_win(c.b_add_rule, f.x, top + lb_h + p(8), half, p(BTN_H));
        move_win(c.b_del_rule, f.x + half + p(8), top + lb_h + p(8), lw - half - p(8), p(BTN_H));
        let rx = f.x + lw + p(16);
        let rw = f.x + f.cw - rx;
        let lw2 = p(64);
        let mut y = top;
        move_win(c.l_season, rx, y, lw2, p(ROW_H));
        move_win(c.cb_season, rx + lw2, y, p(220).min(rw - lw2), p(240));
        y += p(ROW_H) + p(8);
        move_win(c.l_rhours, rx, y, lw2, p(ROW_H));
        move_win(c.dt_rfrom, rx + lw2, y, cw, p(ROW_H));
        move_win(c.s_rto, rx + lw2 + cw + p(10), y, p(20), p(ROW_H));
        move_win(c.dt_rto, rx + lw2 + cw + p(34), y, cw, p(ROW_H));
        y += p(ROW_H) + p(12);
        move_win(c.l_ralbums, rx, y, rw, p(20));
        y += p(22);
        move_win(c.lv_rule, rx, y, rw, (bottom - y).max(p(60)));
        self.size_columns(c.lv_rule, 0);

        // Légende
        let mut f = flow();
        f.check(c.c_place, 0);
        f.gap(4);
        f.check(c.c_date, 0);
        f.gap(12);
        f.combo(c.l_caption, c.cb_caption, 220);
        f.gap(18);
        f.check(c.c_link, 0);
        f.gap(2);
        f.text(c.h_link, CHECK_GLYPH);
        f.gap(18);
        f.check(c.c_lock, 0);
        f.gap(2);
        f.text(c.h_lock, CHECK_GLYPH);

        // Mises à jour
        let mut f = flow();
        f.check(c.c_update, 0);
        f.gap(2);
        show_win(c.h_update_portable, self.portable);
        if self.portable {
            f.text(c.h_update_portable, CHECK_GLYPH);
            f.gap(8);
        }
        f.text(c.h_update, CHECK_GLYPH);
        f.gap(20);
        f.text(c.s_version, 0);
    }

    /// Message de la barre du bas, centré sur la hauteur des boutons.
    fn place_msg(&self) {
        let p = |v| self.px(v);
        let rc = client_rect(self.hwnd);
        let m = p(MARGIN);
        let by = rc.bottom - m - p(BTN_H);
        let x = m + p(128) + p(12);
        let wd = rc.right - m - p(96) - p(8) - p(112) - p(12) - x;
        let ht = text_height(self.c.msg, self.font.get(), wd).min(p(BTN_H) + m);
        move_win(self.c.msg, x, by + (p(BTN_H) - ht) / 2, wd, ht);
    }

    /// La première colonne prend la place laissée par les autres (largeur `other`), barre de
    /// défilement comprise : jamais de défilement horizontal.
    fn size_columns(&self, lv: HWND, other: i32) {
        let cw = client_rect(lv).right;
        if other > 0 {
            send(lv, LVM_SETCOLUMNWIDTH, 1, other as isize);
        }
        send(lv, LVM_SETCOLUMNWIDTH, 0, (cw - other - self.px(20)).max(self.px(80)) as isize);
    }

    fn show_page(&self, i: usize) {
        self.st.borrow_mut().page = i;
        send(self.c.tab, TCM_SETCURSEL, i, 0);
        for (j, pg) in self.c.pages.iter().enumerate() {
            if j == i {
                unsafe {
                    let _ = SetWindowPos(*pg, None, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE);
                }
            }
            show_win(*pg, j == i);
        }
        // Le clavier ne doit pas rester sur un contrôle désormais caché.
        unsafe {
            let f = GetFocus();
            if !f.is_invalid() && self.c.pages.iter().enumerate().any(|(j, pg)| j != i && IsChild(*pg, f).as_bool()) {
                let _ = SetFocus(Some(self.c.tab));
            }
        }
    }

    // ---------- messages ----------

    fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_COMMAND => {
                self.command(wp.0 & 0xFFFF, (wp.0 >> 16) as u32 & 0xFFFF, HWND(lp.0 as *mut _));
                return Some(LRESULT(0));
            }
            WM_NOTIFY if lp.0 != 0 => return Some(LRESULT(self.notify(lp.0))),
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLORDLG => {
                let bg = if hwnd == self.hwnd { COLOR_BTNFACE } else { COLOR_WINDOW };
                let dc = HDC(wp.0 as *mut _);
                unsafe {
                    if let Some(&col) = self.colors.borrow().get(&lp.0) {
                        SetTextColor(dc, COLORREF(col));
                    }
                    SetBkColor(dc, COLORREF(sys_color(bg)));
                    return Some(LRESULT(GetSysColorBrush(bg).0 as isize));
                }
            }
            _ => {}
        }
        if hwnd != self.hwnd {
            return None;
        }
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
                        let _ = SetFocus(Some(HWND(f as *mut _)));
                    }
                }
            },
            WM_DPICHANGED if lp.0 != 0 => {
                self.dpi.set(((wp.0 & 0xFFFF) as u32).max(96));
                let old = self.font.replace(message_font(self.dpi.get()));
                for h in &self.ctls {
                    send(*h, WM_SETFONT, self.font.get().0 as usize, 0);
                }
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old.0));
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

    fn command(&self, id: usize, code: u32, ctl: HWND) {
        let c = &self.c;
        match () {
            _ if id == ID_SAVE => {
                self.save();
            }
            _ if id == ID_CLOSE => self.close(),
            _ if ctl == c.e_url && code == EN_CHANGE => self.update_http_warn(),
            _ if ctl == c.e_filter && code == EN_CHANGE => self.fill_albums(),
            _ if ctl == c.lb_rules && code == LBN_SELCHANGE => {
                let i = send(c.lb_rules, LB_GETCURSEL, 0, 0);
                self.st.borrow_mut().cur = usize::try_from(i).ok();
                self.load_rule();
            }
            _ if ctl == c.cb_season && code == CBN_SELCHANGE => {
                let season = choice_value(SEASONS, combo_sel(c.cb_season));
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
                    self.refresh_rule_label();
                }
            }
            _ if code != BN_CLICKED => {}
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
                    let _ = SetFocus(Some(c.cb_season));
                }
            }
            _ if ctl == c.b_del_rule => {
                {
                    let mut st = self.st.borrow_mut();
                    if let Some(i) = st.cur.filter(|&i| i < st.rules.len()) {
                        st.rules.remove(i);
                    }
                }
                self.fill_rules();
            }
            _ => {}
        }
    }

    fn notify(&self, lp: isize) -> isize {
        let c = &self.c;
        let hdr = unsafe { &*(lp as *const NMHDR) };
        let from = hdr.hwndFrom;
        match hdr.code {
            TCN_SELCHANGE if from == c.tab => {
                let i = send(c.tab, TCM_GETCURSEL, 0, 0);
                if let Ok(i) = usize::try_from(i) {
                    self.show_page(i);
                }
            }
            LVN_ITEMCHANGED if from == c.lv_albums || from == c.lv_rule => {
                if self.filling.get() {
                    return 0;
                }
                let nm = unsafe { &*(lp as *const NMLISTVIEW) };
                let mask = LVIS_STATEIMAGEMASK.0;
                if nm.uChanged.0 & LVIF_STATE.0 == 0 || (nm.uNewState ^ nm.uOldState) & mask == 0 || nm.uOldState & mask == 0 {
                    return 0;
                }
                let Ok(i) = usize::try_from(nm.iItem) else { return 0 };
                let on = nm.uNewState & mask == 2 << 12;
                if from == c.lv_albums {
                    {
                        let mut st = self.st.borrow_mut();
                        if let Some(id) = st.rows.get(i).cloned() {
                            toggle_id(&mut st.selected, &id, on);
                        }
                    }
                    self.update_count();
                } else {
                    {
                        let mut st = self.st.borrow_mut();
                        let id = st.rule_rows.get(i).cloned();
                        if let (Some(cur), Some(id)) = (st.cur, id) {
                            if let Some(r) = st.rules.get_mut(cur) {
                                toggle_id(&mut r.albums, &id, on);
                            }
                        }
                    }
                    self.refresh_rule_label();
                }
            }
            LVN_GETEMPTYMARKUP if from == c.lv_albums || from == c.lv_rule => {
                let text = {
                    let st = self.st.borrow();
                    if from == c.lv_rule && st.cur.is_none() {
                        "Ajoute ou choisis une règle à gauche."
                    } else if !st.albums.is_empty() || (from == c.lv_albums && !st.selected.is_empty()) {
                        "Aucun album ne correspond."
                    } else {
                        "Teste la connexion (onglet Serveur) pour afficher tes albums."
                    }
                };
                let m = unsafe { &mut *(lp as *mut NMLVEMPTYMARKUP) };
                m.dwFlags = EMF_CENTERED;
                let t: Vec<u16> = text.encode_utf16().take(m.szMarkup.len() - 1).chain(std::iter::once(0)).collect();
                m.szMarkup[..t.len()].copy_from_slice(&t);
                return 1;
            }
            DTN_DATETIMECHANGE if from == c.dt_rfrom || from == c.dt_rto => {
                if self.filling.get() {
                    return 0;
                }
                let (f, t) = (clock_value(c.dt_rfrom), clock_value(c.dt_rto));
                let changed = {
                    let mut st = self.st.borrow_mut();
                    match st.cur {
                        Some(i) if i < st.rules.len() => {
                            st.rules[i].from = f;
                            st.rules[i].to = t;
                            true
                        }
                        _ => false,
                    }
                };
                if changed {
                    self.refresh_rule_label();
                }
            }
            _ => {}
        }
        0
    }

    // ---------- contenu ----------

    fn load(&self) {
        let ui = self.settings.load();
        let cfg = &ui.cfg;
        let c = &self.c;
        set_text(c.e_url, &cfg.server_url);
        set_checked(c.c_insecure, cfg.insecure_skip_verify);

        let interval = if cfg.interval_minutes < 1 {
            60
        } else {
            cfg.interval_minutes.min(99999)
        };
        send(c.ud_interval, UDM_SETPOS32, 0, interval as isize);
        set_text(c.e_interval, &interval.to_string());
        set_combo(c.cb_style, choice_index(STYLES, &cfg.style));
        set_combo(c.cb_size, choice_index(SIZES, &cfg.image_size));
        set_combo(c.cb_monitor, choice_index(MONITORS, &cfg.multi_monitor));
        let offline = cfg.offline_cache.clamp(0, 500);
        send(c.ud_offline, UDM_SETPOS32, 0, offline as isize);
        set_text(c.e_offline, &offline.to_string());
        set_checked(c.c_split, cfg.split_wide);
        set_checked(c.c_landscape, cfg.landscape_only);
        set_checked(c.c_start, cfg.change_on_start);
        set_checked(c.c_unlock, cfg.change_on_unlock);

        set_clock(c.dt_from, &cfg.active_from, "22:00");
        set_clock(c.dt_to, &cfg.active_to, "07:00");
        set_checked(c.c_battery, cfg.pause_on_battery);
        set_checked(c.c_metered, cfg.pause_on_metered);
        set_checked(c.c_fullscreen, cfg.pause_on_fullscreen);
        set_checked(c.c_preload, cfg.preload);

        set_checked(c.c_place, cfg.show_place);
        set_checked(c.c_date, cfg.show_date);
        set_combo(c.cb_caption, choice_index(CAPTIONS, &cfg.caption_position));
        set_checked(c.c_link, cfg.desktop_link);
        set_checked(c.c_lock, cfg.lock_screen);

        set_checked(c.c_update, cfg.auto_update);
        let mut version = format!("Version {VERSION}");
        if self.portable {
            set_text(c.c_update, "Vérifier chaque jour si une nouvelle version est publiée sur GitHub");
            version += " (portable)";
        }
        set_text(c.s_version, &version);

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
        self.update_excluded();
        self.fill_albums();
        self.fill_rules();
        self.update_http_warn();
        let clean = self.form_key();
        self.st.borrow_mut().clean = clean;

        if let Some(e) = &ui.error {
            self.set_msg(&format!("config.json illisible : {e}"), COLOR_ERR);
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
            insecure_skip_verify: is_checked(c.c_insecure),
            interval_minutes: num(c.e_interval).filter(|&v| v >= 1).unwrap_or(60),
            style: choice_value(STYLES, combo_sel(c.cb_style)),
            image_size: choice_value(SIZES, combo_sel(c.cb_size)),
            multi_monitor: choice_value(MONITORS, combo_sel(c.cb_monitor)),
            offline_cache: num(c.e_offline).unwrap_or(0).clamp(0, 500),
            split_wide: is_checked(c.c_split),
            landscape_only: is_checked(c.c_landscape),
            change_on_start: is_checked(c.c_start),
            change_on_unlock: is_checked(c.c_unlock),
            active_from: clock_value(c.dt_from),
            active_to: clock_value(c.dt_to),
            pause_on_battery: is_checked(c.c_battery),
            pause_on_metered: is_checked(c.c_metered),
            pause_on_fullscreen: is_checked(c.c_fullscreen),
            preload: is_checked(c.c_preload),
            show_place: is_checked(c.c_place),
            show_date: is_checked(c.c_date),
            caption_position: choice_value(CAPTIONS, combo_sel(c.cb_caption)),
            desktop_link: is_checked(c.c_link),
            lock_screen: is_checked(c.c_lock),
            auto_update: is_checked(c.c_update),
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
                set_text(self.c.e_key, "");
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
                self.set_msg("Enregistré ✓", COLOR_OK);
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
            self.set_test_msg("Connexion…", 0);
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
        self.set_msg("Changement en cours…", 0);
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
        for d in mine {
            match d {
                Done::Albums { quiet, res } => {
                    self.st.borrow_mut().testing = false;
                    enable(self.c.b_test, true);
                    enable(self.c.b_reload, true);
                    match res {
                        Err(e) => self.set_test_msg(&e, COLOR_ERR),
                        Ok(albums) => {
                            let n = albums.len();
                            let goto_albums = {
                                let mut st = self.st.borrow_mut();
                                st.albums = albums;
                                !quiet && st.selected.is_empty() && st.page == PAGE_SERVER
                            };
                            LOADED_ALBUMS.store(n, Ordering::SeqCst);
                            self.set_test_msg(&format!("Connecté — {n} albums"), COLOR_OK);
                            self.fill_albums();
                            self.fill_rule_albums();
                            if goto_albums {
                                self.show_page(PAGE_ALBUMS); // étape suivante : cocher les albums
                            }
                        }
                    }
                }
                Done::Next(res) => {
                    self.st.borrow_mut().changing = false;
                    enable(self.c.b_next, true);
                    match res {
                        Ok(m) => self.set_msg(&m, COLOR_OK),
                        Err(e) => self.set_msg(&e, COLOR_ERR),
                    }
                    self.refresh_status();
                }
            }
        }
    }

    fn clear_excluded(&self) {
        if let Some(h) = self.settings.hooks() {
            if let Err(e) = h.clear_excluded() {
                self.set_msg(&e, COLOR_ERR);
                return;
            }
        }
        self.st.borrow_mut().excluded = 0;
        self.update_excluded();
        self.set_msg("Photos réaffichées", COLOR_OK);
    }

    /// Texte et couleur d'un libellé (couleur 0 : texte normal).
    fn set_colored(&self, h: HWND, text: &str, color: u32) {
        {
            let mut colors = self.colors.borrow_mut();
            if color == 0 {
                colors.remove(&(h.0 as isize));
            } else {
                colors.insert(h.0 as isize, color);
            }
        }
        set_text(h, text);
        invalidate(h);
    }

    fn set_msg(&self, text: &str, color: u32) {
        self.set_colored(self.c.msg, text, color);
        if self.ready.get() {
            self.place_msg();
        }
    }

    fn set_test_msg(&self, text: &str, color: u32) {
        self.set_colored(self.c.test_msg, text, color);
        self.set_colored(self.c.albums_msg, text, color);
        self.layout();
    }

    fn refresh_status(&self) {
        let Some(h) = self.settings.hooks() else { return };
        let s = h.status();
        let changed = {
            let mut st = self.st.borrow_mut();
            let changed = st.last_status != s;
            st.last_status = s.clone();
            changed
        };
        if changed {
            set_text(self.c.status, &s);
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
        let mut s = "Le certificat présenté lors du test est mémorisé (empreinte SHA-256) : seul ce certificat sera \
                     accepté ensuite. S'il est renouvelé, reteste la connexion puis enregistre."
            .to_string();
        let fp = self.st.borrow().cert_fp.clone();
        if fp.len() >= 16 {
            s += &format!(" Empreinte actuelle : {}…", &fp[..16]);
        }
        set_text(self.c.h_pin, &s);
        self.layout();
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
            self.layout();
        }
    }

    fn update_excluded(&self) {
        let n = self.st.borrow().excluded;
        set_text(
            self.c.excluded_info,
            &format!("{n} photo(s) masquée(s) depuis le menu « Ne plus afficher cette photo »."),
        );
        self.layout();
    }

    fn update_count(&self) {
        let n = self.st.borrow().selected.len();
        set_text(self.c.album_count, &count_label(n));
    }

    /// Remplit une liste d'albums à cocher et renvoie les identifiants affichés.
    fn fill_list(&self, lv: HWND, rows: &[AlbumRow], checked: &[String], filter: &str, counts: bool) -> Vec<String> {
        self.filling.set(true);
        send(lv, WM_SETREDRAW, 0, 0);
        send(lv, LVM_DELETEALLITEMS, 0, 0);
        let mut ids = vec![];
        for r in rows {
            if !filter.trim().is_empty() && !matches_filter(&r.name, filter) {
                continue;
            }
            let mut t = wide(&r.name);
            let it = LVITEMW {
                mask: LVIF_TEXT,
                iItem: ids.len() as i32,
                pszText: PWSTR(t.as_mut_ptr()),
                ..Default::default()
            };
            let i = send_ptr(lv, LVM_INSERTITEMW, 0, &it);
            if i < 0 {
                continue;
            }
            if let (true, Some(n)) = (counts, r.count) {
                let mut t = wide(&n.to_string());
                let sub = LVITEMW {
                    iSubItem: 1,
                    pszText: PWSTR(t.as_mut_ptr()),
                    ..Default::default()
                };
                send_ptr(lv, LVM_SETITEMTEXTW, i as usize, &sub);
            }
            let image = if checked.contains(&r.id) { 2 } else { 1 };
            let st = LVITEMW {
                stateMask: LVIS_STATEIMAGEMASK,
                state: LIST_VIEW_ITEM_STATE_FLAGS(image << 12),
                ..Default::default()
            };
            send_ptr(lv, LVM_SETITEMSTATE, i as usize, &st);
            ids.push(r.id.clone());
        }
        send(lv, WM_SETREDRAW, 1, 0);
        invalidate(lv);
        self.filling.set(false);
        ids
    }

    fn fill_albums(&self) {
        let filter = get_text(self.c.e_filter);
        let (rows, checked) = {
            let st = self.st.borrow();
            (album_rows(&st.albums, &st.selected), st.selected.clone())
        };
        let ids = self.fill_list(self.c.lv_albums, &rows, &checked, &filter, true);
        self.st.borrow_mut().rows = ids;
        self.update_count();
    }

    fn fill_rule_albums(&self) {
        let (rows, checked) = {
            let st = self.st.borrow();
            match st.cur.and_then(|i| st.rules.get(i)) {
                Some(r) => (album_rows(&st.albums, &r.albums), r.albums.clone()),
                None => (vec![], vec![]),
            }
        };
        let ids = self.fill_list(self.c.lv_rule, &rows, &checked, "", false);
        self.st.borrow_mut().rule_rows = ids;
    }

    fn fill_rules(&self) {
        let (labels, cur) = {
            let mut st = self.st.borrow_mut();
            let n = st.rules.len();
            st.cur = if n == 0 { None } else { st.cur.map(|c| c.min(n - 1)) };
            (st.rules.iter().map(rule_label).collect::<Vec<_>>(), st.cur)
        };
        send(self.c.lb_rules, LB_RESETCONTENT, 0, 0);
        for l in &labels {
            send_str(self.c.lb_rules, LB_ADDSTRING, 0, l);
        }
        if let Some(i) = cur {
            send(self.c.lb_rules, LB_SETCURSEL, i, 0);
        }
        self.load_rule();
    }

    fn refresh_rule_label(&self) {
        let (i, label) = {
            let st = self.st.borrow();
            match st.cur.and_then(|i| st.rules.get(i).map(|r| (i, rule_label(r)))) {
                Some(v) => v,
                None => return,
            }
        };
        send(self.c.lb_rules, LB_DELETESTRING, i, 0);
        send_str(self.c.lb_rules, LB_INSERTSTRING, i, &label);
        send(self.c.lb_rules, LB_SETCURSEL, i, 0);
    }

    /// Affiche la règle sélectionnée dans les champs de droite.
    fn load_rule(&self) {
        let c = &self.c;
        let rule = {
            let st = self.st.borrow();
            st.cur.and_then(|i| st.rules.get(i).cloned())
        };
        for h in [
            c.l_season,
            c.cb_season,
            c.l_rhours,
            c.dt_rfrom,
            c.s_rto,
            c.dt_rto,
            c.l_ralbums,
            c.lv_rule,
            c.b_del_rule,
        ] {
            enable(h, rule.is_some());
        }
        self.filling.set(true);
        let r = rule.unwrap_or_default();
        set_combo(c.cb_season, choice_index(SEASONS, &r.season));
        set_clock(c.dt_rfrom, &r.from, "08:00");
        set_clock(c.dt_rto, &r.to, "20:00");
        self.filling.set(false);
        self.fill_rule_albums();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{read_config, save_config};
    use crate::testutil::{temp_dir, FakeImmich, ALB1, ALB2, KEY};
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE};

    const LVM_GETITEMCOUNT: u32 = 0x1004;
    const LVM_GETITEMSTATE: u32 = 0x102C;

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

    fn row_count(lv: HWND) -> usize {
        send(lv, LVM_GETITEMCOUNT, 0, 0) as usize
    }

    fn set_row_checked(lv: HWND, i: usize, on: bool) {
        let st = LVITEMW {
            stateMask: LVIS_STATEIMAGEMASK,
            state: LIST_VIEW_ITEM_STATE_FLAGS(if on { 2 << 12 } else { 1 << 12 }),
            ..Default::default()
        };
        send_ptr(lv, LVM_SETITEMSTATE, i, &st);
    }

    fn row_checked(lv: HWND, i: usize) -> bool {
        send(lv, LVM_GETITEMSTATE, i, LVIS_STATEIMAGEMASK.0 as isize) as u32 == 2 << 12
    }

    fn click(w: &Win, h: HWND) {
        send(w.hwnd, WM_COMMAND, (BN_CLICKED as usize) << 16, h.0 as isize);
    }

    /// Fenêtre remplie depuis la configuration, relue à l'identique ; listes d'albums, règles,
    /// enregistrement, erreurs, détection des modifications et changement de résolution.
    #[test]
    fn settings_window_round_trip() {
        let fake = FakeImmich::start();
        let cfg_path = temp_dir("settings_window").join("config.json");
        let cfg = Config {
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
        };
        save_config(&cfg_path, &cfg).unwrap();
        let settings = Settings::new(&cfg_path);
        let w = Win::create(settings).expect("création de la fenêtre");
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

        // Le test de connexion lancé à l'ouverture remplit les listes.
        assert!(pump_until(20, || loaded_albums() > 0), "albums non chargés");
        let n = w.st.borrow().albums.len();
        assert_eq!(row_count(w.c.lv_albums), n);
        let rows = w.st.borrow().rows.clone();
        let i1 = rows.iter().position(|r| r == ALB1).expect("album 1 affiché");
        let i2 = rows.iter().position(|r| r == ALB2).expect("album 2 affiché");
        assert!(row_checked(w.c.lv_albums, i2) && !row_checked(w.c.lv_albums, i1));
        assert_eq!(get_text(w.c.album_count), "1 album sélectionné");

        // Cocher un album passe par la notification de la liste.
        set_row_checked(w.c.lv_albums, i1, true);
        assert_eq!(w.st.borrow().selected, vec![ALB2.to_string(), ALB1.to_string()]);
        assert!(w.dirty());
        // Le filtre ne perd pas les cases cochées.
        set_text(w.c.e_filter, "zzz-aucun");
        assert_eq!(row_count(w.c.lv_albums), 0);
        set_text(w.c.e_filter, "");
        assert_eq!(row_count(w.c.lv_albums), n);
        assert_eq!(w.form().albums.len(), 2);

        // Règles : liste, sélection, albums de la règle.
        assert_eq!(send(w.c.lb_rules, LB_GETCURSEL, 0, 0), 0);
        let rr = w.st.borrow().rule_rows.clone();
        let r1 = rr.iter().position(|r| r == ALB1).unwrap();
        let r2 = rr.iter().position(|r| r == ALB2).unwrap();
        assert!(row_checked(w.c.lv_rule, r1));
        set_row_checked(w.c.lv_rule, r2, true);
        assert_eq!(w.form().album_rules[0].albums.len(), 2);
        click(&w, w.c.b_add_rule);
        assert_eq!(w.st.borrow().rules.len(), 2);
        assert_eq!(w.form().album_rules.len(), 1, "règle sans album ignorée");
        click(&w, w.c.b_del_rule);
        assert_eq!(w.st.borrow().rules.len(), 1);

        // Enregistrement : le token enregistré est conservé.
        set_text(w.c.e_interval, "7");
        set_combo(w.c.cb_style, choice_index(STYLES, "span"));
        assert!(w.save());
        assert!(!w.dirty());
        let saved = read_config(&cfg_path).unwrap();
        assert_eq!(saved.api_key, KEY);
        assert_eq!(saved.albums, vec![ALB2.to_string(), ALB1.to_string()]);
        assert_eq!(saved.interval_minutes, 7);
        assert_eq!(saved.style, "span");
        assert_eq!(saved.album_rules[0].albums.len(), 2);
        assert_eq!((saved.active_from.as_str(), saved.active_to.as_str()), ("22:00", "07:30"));

        // Erreur de validation : expliquée, rien n'est enregistré.
        set_text(w.c.e_url, "pas une url");
        assert!(!w.save());
        assert_eq!(alerts.borrow().len(), 1);
        assert!(alerts.borrow()[0].contains("saisis de nouveau le token"), "{:?}", alerts.borrow());
        set_text(w.c.e_key, "autre-token");
        assert!(!w.save());
        assert_eq!(alerts.borrow().len(), 2);
        assert!(alerts.borrow()[1].contains("server_url"), "{:?}", alerts.borrow());
        set_text(w.c.e_key, "");
        assert_eq!(read_config(&cfg_path).unwrap().server_url, fake.url);
        // Avertissement http:// vers Internet.
        set_text(w.c.e_url, "http://immich.exemple.fr");
        assert!(w.st.borrow().http_warn);
        set_text(w.c.e_url, &fake.url);
        assert!(!w.st.borrow().http_warn && !w.dirty());

        // Changement de résolution (écran à 150 %) : police et mise en page recalculées.
        let before = client_rect(w.c.e_url).bottom;
        let r = RECT {
            left: 0,
            top: 0,
            right: 1050,
            bottom: 870,
        };
        send(w.hwnd, WM_DPICHANGED, (144 << 16) | 144, &r as *const RECT as isize);
        assert_eq!(w.dpi.get(), 144);
        assert!(client_rect(w.c.e_url).bottom > before, "zone de saisie non agrandie");

        // Fermeture avec modifications : « Annuler » garde la fenêtre, « Non » la ferme.
        set_checked(w.c.c_lock, false);
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

    /// Captures de chaque onglet (IW_SETTINGS_SHOTS=<dossier>), pour vérifier la mise en page.
    #[test]
    fn settings_window_screenshots() {
        let Some(dir) = std::env::var_os("IW_SETTINGS_SHOTS") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = FakeImmich::start();
        let cfg_path = temp_dir("settings_shots").join("config.json");
        let cfg = Config {
            server_url: fake.url.clone(),
            api_key: KEY.into(),
            albums: vec![ALB2.into()],
            album_rules: vec![AlbumRule {
                albums: vec![ALB1.into()],
                season: "summer".into(),
                from: "08:00".into(),
                to: "20:00".into(),
            }],
            ..Config::default()
        };
        save_config(&cfg_path, &cfg).unwrap();
        let w = Win::create(Settings::new(&cfg_path)).unwrap();
        w.show();
        assert!(pump_until(20, || loaded_albums() > 0));
        for i in 0..PAGE_NAMES.len() {
            w.show_page(i);
            pump_until(1, || false);
            shot(w.hwnd, &dir.join(format!("onglet-{i}.png")));
        }
        *w.alert.borrow_mut() = Some(Box::new(|_, _| IDNO.0));
        unsafe {
            let _ = DestroyWindow(w.hwnd);
        }
    }

    /// Capture de la fenêtre (copie de l'écran à son emplacement).
    fn shot(hwnd: HWND, path: &std::path::Path) {
        use windows::Win32::Graphics::Gdi::*;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
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
