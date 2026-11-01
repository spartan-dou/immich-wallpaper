package main

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

var (
	user32      = windows.NewLazySystemDLL("user32.dll")
	procSPI     = user32.NewProc("SystemParametersInfoW")
	procMsgBoxW = user32.NewProc("MessageBoxW")
)

const (
	spiSetDeskWallpaper = 0x0014
	spifUpdateIniFile   = 0x01
	spifSendChange      = 0x02
	runKey              = `Software\Microsoft\Windows\CurrentVersion\Run`
	runValue            = "ImmichWallpaper"
)

func setWallpaper(path, style string) error {
	ws, tile, ok := styleValues(style)
	if !ok {
		ws, tile = "10", "0"
	}
	k, _, err := registry.CreateKey(registry.CURRENT_USER, `Control Panel\Desktop`, registry.SET_VALUE)
	if err == nil {
		k.SetStringValue("WallpaperStyle", ws)
		k.SetStringValue("TileWallpaper", tile)
		k.Close()
	}
	// Repasse l'arrière-plan en mode « Image » si « Windows à la une » ou un diaporama était actif,
	// sinon Windows peut remettre ses propres images par-dessus.
	if k, err := registry.OpenKey(registry.CURRENT_USER, `Software\Microsoft\Windows\CurrentVersion\Explorer\Wallpapers`, registry.QUERY_VALUE|registry.SET_VALUE); err == nil {
		if v, _, err := k.GetIntegerValue("BackgroundType"); err == nil && v != 0 {
			k.SetDWordValue("BackgroundType", 0)
		}
		k.Close()
	}
	p, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return err
	}
	r, _, e := procSPI.Call(spiSetDeskWallpaper, 0, uintptr(unsafe.Pointer(p)), spifUpdateIniFile|spifSendChange)
	if r == 0 {
		return fmt.Errorf("SystemParametersInfo : %v", e)
	}
	return nil
}

func msgBox(title, text string, flags uintptr) {
	t, _ := windows.UTF16PtrFromString(title)
	m, _ := windows.UTF16PtrFromString(text)
	procMsgBoxW.Call(0, uintptr(unsafe.Pointer(m)), uintptr(unsafe.Pointer(t)), flags)
}

func shellOpen(target string) {
	verb, _ := windows.UTF16PtrFromString("open")
	t, _ := windows.UTF16PtrFromString(target)
	windows.ShellExecute(0, verb, t, nil, nil, windows.SW_SHOWNORMAL)
}

func autostartEnabled() bool {
	k, err := registry.OpenKey(registry.CURRENT_USER, runKey, registry.QUERY_VALUE)
	if err != nil {
		return false
	}
	defer k.Close()
	_, _, err = k.GetStringValue(runValue)
	return err == nil
}

func setAutostart(on bool) error {
	k, _, err := registry.CreateKey(registry.CURRENT_USER, runKey, registry.SET_VALUE)
	if err != nil {
		return err
	}
	defer k.Close()
	if !on {
		err := k.DeleteValue(runValue)
		if err == registry.ErrNotExist {
			return nil
		}
		return err
	}
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	return k.SetStringValue(runValue, `"`+exe+`"`)
}

// singleInstance renvoie false si l'appli tourne déjà.
func singleInstance() bool {
	name, _ := windows.UTF16PtrFromString(`Local\ImmichWallpaperSingleton`)
	_, err := windows.CreateMutex(nil, false, name)
	return err != windows.ERROR_ALREADY_EXISTS
}

var procGetSystemMetrics = user32.NewProc("GetSystemMetrics")

func metric(i uintptr) int {
	r, _, _ := procGetSystemMetrics.Call(i)
	return int(int32(r))
}

// screenFor renvoie la taille de l'écran principal (ou du bureau virtuel en mode span),
// en pixels physiques grâce au manifeste PerMonitorV2.
func screenFor(style string) Screen {
	if style == "span" {
		return Screen{W: metric(78), H: metric(79)}
	}
	sc := Screen{W: metric(0), H: metric(1)}
	var rc struct{ L, T, R, B int32 }
	if r, _, _ := procSPI.Call(0x0030 /* SPI_GETWORKAREA */, 0, uintptr(unsafe.Pointer(&rc)), 0); r != 0 {
		if inset := sc.H - int(rc.B); inset > 0 && inset < sc.H/4 {
			sc.BottomInset = inset
		}
	}
	return sc
}

// Script PowerShell appelant l'API WinRT Windows.System.UserProfile.LockScreen.
const lockScreenPS = `$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$m = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 }
$asOp = $m | Where-Object { $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation` + "`" + `1' } | Select-Object -First 1
$asAct = $m | Where-Object { $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction' } | Select-Object -First 1
[Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime] | Out-Null
[Windows.System.UserProfile.LockScreen, Windows.System.UserProfile, ContentType = WindowsRuntime] | Out-Null
$t = $asOp.MakeGenericMethod([Windows.Storage.StorageFile]).Invoke($null, @([Windows.Storage.StorageFile]::GetFileFromPathAsync($env:IW_LOCK_PATH)))
$t.Wait(-1) | Out-Null
$t2 = $asAct.Invoke($null, @([Windows.System.UserProfile.LockScreen]::SetImageFileAsync($t.Result)))
$t2.Wait(-1) | Out-Null
`

func setLockScreen(path string) error {
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, "powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", lockScreenPS)
	cmd.Env = append(os.Environ(), "IW_LOCK_PATH="+path)
	cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: 0x08000000 /* CREATE_NO_WINDOW */}
	out, err := cmd.CombinedOutput()
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if len(msg) > 400 {
			msg = msg[:400]
		}
		return fmt.Errorf("%v : %s", err, msg)
	}
	return nil
}

const desktopLinkName = "Voir cette photo dans Immich.url"

func desktopLinkPath() string {
	dir, err := windows.KnownFolderPath(windows.FOLDERID_Desktop, 0) // gère le bureau redirigé (OneDrive…)
	if err != nil {
		home, _ := os.UserHomeDir()
		dir = home + `\Desktop`
	}
	return dir + `\` + desktopLinkName
}

// updateDesktopLink écrit (ou supprime) le raccourci du bureau vers la photo affichée,
// à la manière de l'icône « En savoir plus sur cette image » de Windows à la une.
func updateDesktopLink(cfg *Config, a Asset, webURL string) {
	path := desktopLinkPath()
	if !cfg.DesktopLink {
		os.Remove(path)
		return
	}
	exe, _ := os.Executable()
	content := "[InternetShortcut]\r\nURL=" + webURL + "\r\nIconFile=" + exe + "\r\nIconIndex=0\r\n"
	os.WriteFile(path, []byte(content), 0o644)
}

func removeDesktopLink() { os.Remove(desktopLinkPath()) }
