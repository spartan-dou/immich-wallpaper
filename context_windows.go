package main

import (
	"runtime"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

// État du PC consulté avant un changement automatique : alimentation, plein écran, connexion limitée.

var (
	procGetSystemPowerStatus         = windows.NewLazySystemDLL("kernel32.dll").NewProc("GetSystemPowerStatus")
	procSHQueryUserNotificationState = windows.NewLazySystemDLL("shell32.dll").NewProc("SHQueryUserNotificationState")

	clsidNetworkListManager, _ = windows.GUIDFromString("{DCB00C01-570F-4A9B-8D69-199FDBA5723B}")
	iidNetworkCostManager, _   = windows.GUIDFromString("{DCB00008-570F-4A9B-8D69-199FDBA5723B}")
)

// onBattery : le PC fonctionne sur batterie (faux sur un PC fixe ou en cas de doute).
func onBattery() bool {
	var st struct {
		ACLineStatus, BatteryFlag, BatteryLifePercent, SystemStatusFlag byte
		BatteryLifeTime, BatteryFullLifeTime                            uint32
	}
	r, _, _ := procGetSystemPowerStatus.Call(uintptr(unsafe.Pointer(&st)))
	return r != 0 && st.ACLineStatus == 0
}

// fullscreenBusy : une application occupe tout l'écran (jeu, vidéo, présentation).
func fullscreenBusy() bool {
	var state int32
	if r, _, _ := procSHQueryUserNotificationState.Call(uintptr(unsafe.Pointer(&state))); int32(r) < 0 {
		return false
	}
	// QUNS_BUSY (2), QUNS_RUNNING_D3D_FULL_SCREEN (3), QUNS_PRESENTATION_MODE (4)
	return state >= 2 && state <= 4
}

// connectionCost interroge INetworkCostManager::GetCost (coût de la connexion Internet active).
func connectionCost() (cost uint32, err error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	if err := windows.CoInitializeEx(0, windows.COINIT_APARTMENTTHREADED); err != nil {
		if e, ok := err.(syscall.Errno); !ok || e != 1 /* S_FALSE : déjà initialisé */ {
			return 0, err
		}
	}
	defer windows.CoUninitialize()
	var obj unsafe.Pointer
	r, _, _ := procCoCreateInstance.Call(
		uintptr(unsafe.Pointer(&clsidNetworkListManager)), 0, 0x17, /* CLSCTX_ALL */
		uintptr(unsafe.Pointer(&iidNetworkCostManager)), uintptr(unsafe.Pointer(&obj)))
	if int32(r) < 0 || obj == nil {
		return 0, syscall.Errno(r)
	}
	vtbl := *(**[6]uintptr)(obj) // IUnknown (0-2), GetCost (3)
	defer syscall.SyscallN(vtbl[2], uintptr(obj))
	if r, _, _ := syscall.SyscallN(vtbl[3], uintptr(obj), uintptr(unsafe.Pointer(&cost)), 0); int32(r) < 0 {
		return 0, syscall.Errno(r)
	}
	return cost, nil
}

// meteredConnection : connexion limitée ou facturée à l'usage (partage de connexion,
// forfait de données, itinérance). Faux si l'information est indisponible.
func meteredConnection() bool {
	cost, err := connectionCost()
	if err != nil {
		return false
	}
	// NLM_CONNECTION_COST_FIXED (2), _VARIABLE (4), _OVERDATALIMIT (0x10000), _ROAMING (0x40000)
	return cost&(0x2|0x4|0x10000|0x40000) != 0
}
