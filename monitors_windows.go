package main

import (
	"fmt"
	"runtime"
	"sort"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Interface COM IDesktopWallpaper (Windows 8+) : un fond d'écran par écran.
var (
	ole32                = windows.NewLazySystemDLL("ole32.dll")
	procCoCreateInstance = ole32.NewProc("CoCreateInstance")
	procMonitorFromRect  = user32.NewProc("MonitorFromRect")
	procGetMonitorInfoW  = user32.NewProc("GetMonitorInfoW")

	clsidDesktopWallpaper, _ = windows.GUIDFromString("{C2CF3110-460E-4FC1-B9D0-8A1C0C9CC4BD}")
	iidDesktopWallpaper, _   = windows.GUIDFromString("{B92B56A9-8B55-4E14-9A89-0199BBB6F93B}")
)

// Indices dans la vtable d'IDesktopWallpaper (après les 3 méthodes d'IUnknown).
const (
	dwRelease               = 2
	dwSetWallpaper          = 3
	dwGetMonitorDevicePath  = 5
	dwGetMonitorDeviceCount = 6
	dwGetMonitorRECT        = 7
	dwSetPosition           = 10
)

type rect32 struct{ L, T, R, B int32 }

type desktopWallpaper struct{ obj unsafe.Pointer }

func (d desktopWallpaper) call(idx int, args ...uintptr) error {
	vtbl := *(**[20]uintptr)(d.obj)
	r, _, _ := syscall.SyscallN(vtbl[idx], append([]uintptr{uintptr(d.obj)}, args...)...)
	if int32(r) < 0 {
		return fmt.Errorf("IDesktopWallpaper : HRESULT 0x%08X", uint32(r))
	}
	return nil
}

// withDesktopWallpaper initialise COM sur un thread fixe, crée l'objet et appelle f.
func withDesktopWallpaper(f func(d desktopWallpaper) error) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	if err := windows.CoInitializeEx(0, windows.COINIT_APARTMENTTHREADED); err != nil {
		if e, ok := err.(syscall.Errno); !ok || e != 1 /* S_FALSE : déjà initialisé */ {
			return err
		}
	}
	defer windows.CoUninitialize()
	var obj unsafe.Pointer
	r, _, _ := procCoCreateInstance.Call(
		uintptr(unsafe.Pointer(&clsidDesktopWallpaper)), 0, 0x17, /* CLSCTX_ALL */
		uintptr(unsafe.Pointer(&iidDesktopWallpaper)), uintptr(unsafe.Pointer(&obj)))
	if int32(r) < 0 || obj == nil {
		return fmt.Errorf("CoCreateInstance(DesktopWallpaper) : 0x%08X", uint32(r))
	}
	d := desktopWallpaper{obj}
	defer d.call(dwRelease)
	return f(d)
}

func workAreaInset(rc rect32) int {
	var mi struct {
		Size          uint32
		Monitor, Work rect32
		Flags         uint32
	}
	mi.Size = uint32(unsafe.Sizeof(mi))
	h, _, _ := procMonitorFromRect.Call(uintptr(unsafe.Pointer(&rc)), 2 /* MONITOR_DEFAULTTONEAREST */)
	if h == 0 {
		return 0
	}
	if r, _, _ := procGetMonitorInfoW.Call(h, uintptr(unsafe.Pointer(&mi))); r == 0 {
		return 0
	}
	inset := int(mi.Monitor.B - mi.Work.B)
	if inset < 0 || inset > int(mi.Monitor.B-mi.Monitor.T)/4 {
		return 0
	}
	return inset
}

// listMonitors renvoie les écrans actifs, l'écran principal en premier.
func listMonitors() []Monitor {
	var mons []Monitor
	var primaryID string
	withDesktopWallpaper(func(d desktopWallpaper) error {
		var n uint32
		if err := d.call(dwGetMonitorDeviceCount, uintptr(unsafe.Pointer(&n))); err != nil {
			return err
		}
		type m struct {
			Monitor
			left int32
		}
		var list []m
		for i := uint32(0); i < n; i++ {
			var p *uint16
			if d.call(dwGetMonitorDevicePath, uintptr(i), uintptr(unsafe.Pointer(&p))) != nil || p == nil {
				continue
			}
			id := windows.UTF16PtrToString(p)
			windows.CoTaskMemFree(unsafe.Pointer(p))
			var rc rect32
			ip, _ := windows.UTF16PtrFromString(id)
			if d.call(dwGetMonitorRECT, uintptr(unsafe.Pointer(ip)), uintptr(unsafe.Pointer(&rc))) != nil {
				continue // écran inactif
			}
			w, h := int(rc.R-rc.L), int(rc.B-rc.T)
			if w <= 0 || h <= 0 { // écran déconnecté
				continue
			}
			if rc.L == 0 && rc.T == 0 {
				primaryID = id
			}
			list = append(list, m{Monitor{ID: id, Screen: Screen{W: w, H: h, BottomInset: workAreaInset(rc)}}, rc.L})
		}
		sort.SliceStable(list, func(i, j int) bool {
			if (list[i].ID == primaryID) != (list[j].ID == primaryID) {
				return list[i].ID == primaryID
			}
			return list[i].left < list[j].left
		})
		for _, x := range list {
			mons = append(mons, x.Monitor)
		}
		return nil
	})
	return mons
}

var dwPositions = map[string]uintptr{"center": 0, "tile": 1, "stretch": 2, "fit": 3, "fill": 4, "span": 5}

// setMonitorWallpapers applique une image par écran (chemin de périphérique → fichier).
func setMonitorWallpapers(paths map[string]string, style string) error {
	setBackgroundTypePicture()
	return withDesktopWallpaper(func(d desktopWallpaper) error {
		pos, ok := dwPositions[style]
		if !ok {
			pos = 4
		}
		d.call(dwSetPosition, pos)
		for id, path := range paths {
			ip, _ := windows.UTF16PtrFromString(id)
			pp, _ := windows.UTF16PtrFromString(path)
			if err := d.call(dwSetWallpaper, uintptr(unsafe.Pointer(ip)), uintptr(unsafe.Pointer(pp))); err != nil {
				return err
			}
		}
		return nil
	})
}
