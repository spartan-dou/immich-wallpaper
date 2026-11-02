package main

import (
	"log"
	"runtime"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Surveillance des événements système via une fenêtre cachée (les messages diffusés comme
// WM_DISPLAYCHANGE ne parviennent qu'aux fenêtres de premier niveau, pas aux fenêtres « message-only »).

var (
	procRegisterClassExW          = user32.NewProc("RegisterClassExW")
	procCreateWindowExW           = user32.NewProc("CreateWindowExW")
	procDefWindowProcW            = user32.NewProc("DefWindowProcW")
	procGetMessageW               = user32.NewProc("GetMessageW")
	procTranslateMessage          = user32.NewProc("TranslateMessage")
	procDispatchMessageW          = user32.NewProc("DispatchMessageW")
	procRegisterPowerSettingNotif = user32.NewProc("RegisterPowerSettingNotification")
	wtsapi32                      = windows.NewLazySystemDLL("wtsapi32.dll")
	procWTSRegisterSessionNotif   = wtsapi32.NewProc("WTSRegisterSessionNotification")
	procRtlMoveMemory             = windows.NewLazySystemDLL("kernel32.dll").NewProc("RtlMoveMemory")
	guidConsoleDisplayState, _    = windows.GUIDFromString("{6FE69556-704A-47A0-8F24-C28D936FDA47}")
)

const (
	wmDisplayChange       = 0x007E
	wmWTSSessionChange    = 0x02B1
	wmPowerBroadcast      = 0x0218
	wtsSessionUnlock      = 0x8
	pbtPowerSettingChange = 0x8013
	pbtAPMResumeAutomatic = 0x0012
)

type wndClassEx struct {
	Size       uint32
	Style      uint32
	WndProc    uintptr
	ClsExtra   int32
	WndExtra   int32
	Instance   uintptr
	Icon       uintptr
	Cursor     uintptr
	Background uintptr
	MenuName   *uint16
	ClassName  *uint16
	IconSm     uintptr
}

type winMsg struct {
	Hwnd    uintptr
	Message uint32
	WParam  uintptr
	LParam  uintptr
	Time    uint32
	X, Y    int32
	Private uint32
}

type sysEvents struct {
	onDisplay func() // configuration d'écrans modifiée ou écran rallumé
	onUnlock  func() // session déverrouillée
}

var events *sysEvents

func wndProc(hwnd, msg, wParam, lParam uintptr) uintptr {
	switch msg {
	case wmDisplayChange:
		events.onDisplay()
	case wmWTSSessionChange:
		if wParam == wtsSessionUnlock {
			events.onUnlock()
		}
	case wmPowerBroadcast:
		switch wParam {
		case pbtAPMResumeAutomatic: // sortie de veille
			events.onDisplay()
		case pbtPowerSettingChange:
			// POWERBROADCAST_SETTING : GUID (16) + longueur (4) + données ; 1 = écran allumé.
			var buf [21]byte
			procRtlMoveMemory.Call(uintptr(unsafe.Pointer(&buf[0])), lParam, uintptr(len(buf)))
			if *(*windows.GUID)(unsafe.Pointer(&buf[0])) == guidConsoleDisplayState && buf[20] == 1 {
				events.onDisplay()
			}
		}
		return 1
	}
	r, _, _ := procDefWindowProcW.Call(hwnd, msg, wParam, lParam)
	return r
}

// watchSystemEvents crée la fenêtre cachée et traite ses messages (bloquant : à lancer dans une goroutine).
func watchSystemEvents(ev *sysEvents) {
	runtime.LockOSThread()
	events = ev
	cls, _ := windows.UTF16PtrFromString("ImmichWallpaperEvents")
	var hinst windows.Handle
	windows.GetModuleHandleEx(0, nil, &hinst)
	wc := wndClassEx{WndProc: syscall.NewCallback(wndProc), Instance: uintptr(hinst), ClassName: cls}
	wc.Size = uint32(unsafe.Sizeof(wc))
	if r, _, err := procRegisterClassExW.Call(uintptr(unsafe.Pointer(&wc))); r == 0 {
		log.Printf("événements système indisponibles (RegisterClassEx) : %v", err)
		return
	}
	// Fenêtre de premier niveau jamais affichée (style WS_POPUP, taille nulle).
	hwnd, _, err := procCreateWindowExW.Call(0x80 /* WS_EX_TOOLWINDOW */, uintptr(unsafe.Pointer(cls)), uintptr(unsafe.Pointer(cls)),
		0x80000000 /* WS_POPUP */, 0, 0, 0, 0, 0, 0, uintptr(hinst), 0)
	if hwnd == 0 {
		log.Printf("événements système indisponibles (CreateWindowEx) : %v", err)
		return
	}
	if r, _, err := procWTSRegisterSessionNotif.Call(hwnd, 0 /* NOTIFY_FOR_THIS_SESSION */); r == 0 {
		log.Printf("notification de déverrouillage indisponible : %v", err)
	}
	procRegisterPowerSettingNotif.Call(hwnd, uintptr(unsafe.Pointer(&guidConsoleDisplayState)), 0 /* DEVICE_NOTIFY_WINDOW_HANDLE */)

	var m winMsg
	for {
		r, _, _ := procGetMessageW.Call(uintptr(unsafe.Pointer(&m)), 0, 0, 0)
		if int32(r) <= 0 {
			return
		}
		procTranslateMessage.Call(uintptr(unsafe.Pointer(&m)))
		procDispatchMessageW.Call(uintptr(unsafe.Pointer(&m)))
	}
}
