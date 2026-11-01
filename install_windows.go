package main

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

// appVersion est remplacée au build par la pipeline (-X main.appVersion=...).
var appVersion = "1.4"

const (
	uninstallKey = `Software\Microsoft\Windows\CurrentVersion\Uninstall\ImmichWallpaper`
	mbYesNo      = 0x4
	mbIconQuest  = 0x20
	idYes        = 6
)

func installDir() string {
	base := os.Getenv("LOCALAPPDATA")
	if base == "" {
		base, _ = os.UserConfigDir()
	}
	return filepath.Join(base, "Programs", "ImmichWallpaper")
}

func installedExe() string { return filepath.Join(installDir(), "ImmichWallpaper.exe") }

func dataDir() string {
	base, err := os.UserConfigDir()
	if err != nil {
		base = os.TempDir()
	}
	return filepath.Join(base, "ImmichWallpaper")
}

func startMenuShortcut() string {
	return filepath.Join(os.Getenv("APPDATA"), `Microsoft\Windows\Start Menu\Programs\Immich Wallpaper.lnk`)
}

func samePath(a, b string) bool {
	a, _ = filepath.Abs(a)
	b, _ = filepath.Abs(b)
	return strings.EqualFold(filepath.Clean(a), filepath.Clean(b))
}

func isInstalled() bool {
	_, err := os.Stat(installedExe())
	return err == nil
}

func confirm(text string) bool {
	t, _ := windows.UTF16PtrFromString("Immich Wallpaper")
	m, _ := windows.UTF16PtrFromString(text)
	r, _, _ := procMsgBoxW.Call(0, uintptr(unsafe.Pointer(m)), uintptr(unsafe.Pointer(t)), mbYesNo|mbIconQuest)
	return r == idYes
}

func hiddenCmd(name string, args ...string) *exec.Cmd {
	cmd := exec.Command(name, args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: 0x08000000}
	return cmd
}

// --- instance en cours (pour la mise à jour et la désinstallation) ---

func pidFile() string { return filepath.Join(dataDir(), "instance.pid") }

func writePID() { os.WriteFile(pidFile(), []byte(strconv.Itoa(os.Getpid())), 0o644) }

// stopRunningInstance termine l'autre instance (vérifie que le PID est bien Immich Wallpaper).
func stopRunningInstance() {
	stopByPID()
	// Filet de sécurité (anciennes versions sans fichier PID) : toute autre instance du même nom.
	hiddenCmd("taskkill", "/F", "/IM", "ImmichWallpaper*", "/FI", "PID ne "+strconv.Itoa(os.Getpid())).Run()
	time.Sleep(300 * time.Millisecond)
}

func stopByPID() {
	b, err := os.ReadFile(pidFile())
	if err != nil {
		return
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(b)))
	if err != nil || pid == os.Getpid() {
		return
	}
	h, err := windows.OpenProcess(windows.PROCESS_TERMINATE|windows.SYNCHRONIZE|windows.PROCESS_QUERY_LIMITED_INFORMATION, false, uint32(pid))
	if err != nil {
		return
	}
	defer windows.CloseHandle(h)
	buf := make([]uint16, windows.MAX_PATH)
	n := uint32(len(buf))
	if windows.QueryFullProcessImageName(h, 0, &buf[0], &n) != nil ||
		!strings.HasPrefix(strings.ToLower(filepath.Base(windows.UTF16ToString(buf[:n]))), "immichwallpaper") {
		return
	}
	windows.TerminateProcess(h, 0)
	windows.WaitForSingleObject(h, 5000)
	time.Sleep(300 * time.Millisecond) // laisse Windows libérer le fichier
}

// --- installation ---

func copyFile(src, dst string) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	tmp := dst + ".new"
	out, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	out.Close()
	return os.Rename(tmp, dst)
}

func install() error {
	self, err := os.Executable()
	if err != nil {
		return err
	}
	stopRunningInstance()
	if err := os.MkdirAll(installDir(), 0o755); err != nil {
		return err
	}
	dst := installedExe()
	var cerr error
	for i := 0; i < 5; i++ { // l'ancien exe peut rester verrouillé un court instant
		if cerr = copyFile(self, dst); cerr == nil {
			break
		}
		time.Sleep(time.Second)
	}
	if cerr != nil {
		return cerr
	}

	k, _, err := registry.CreateKey(registry.CURRENT_USER, uninstallKey, registry.SET_VALUE)
	if err != nil {
		return err
	}
	size := uint32(0)
	if st, err := os.Stat(dst); err == nil {
		size = uint32(st.Size() / 1024)
	}
	k.SetStringValue("DisplayName", "Immich Wallpaper")
	k.SetStringValue("DisplayVersion", appVersion)
	k.SetStringValue("Publisher", "Immich Wallpaper")
	k.SetStringValue("DisplayIcon", dst)
	k.SetStringValue("InstallLocation", installDir())
	k.SetStringValue("UninstallString", `"`+dst+`" --uninstall`)
	k.SetStringValue("QuietUninstallString", `"`+dst+`" --uninstall --quiet`)
	k.SetDWordValue("NoModify", 1)
	k.SetDWordValue("NoRepair", 1)
	k.SetDWordValue("EstimatedSize", size)
	k.Close()

	// Raccourci menu Démarrer + démarrage automatique (pointant vers la copie installée).
	ps := `$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:IW_LNK);$s.TargetPath=$env:IW_EXE;$s.WorkingDirectory=$env:IW_DIR;$s.Save()`
	cmd := hiddenCmd("powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", ps)
	cmd.Env = append(os.Environ(), "IW_LNK="+startMenuShortcut(), "IW_EXE="+dst, "IW_DIR="+installDir())
	cmd.Run()

	return setAutostartPath(dst)
}

func setAutostartPath(exe string) error {
	k, _, err := registry.CreateKey(registry.CURRENT_USER, runKey, registry.SET_VALUE)
	if err != nil {
		return err
	}
	defer k.Close()
	return k.SetStringValue(runValue, `"`+exe+`"`)
}

// maybeInstall propose l'installation (ou la mise à jour) quand l'exe est lancé hors du dossier d'installation.
// Renvoie true si le processus courant doit s'arrêter (la copie installée a été lancée).
func maybeInstall() bool {
	self, err := os.Executable()
	if err != nil || samePath(self, installedExe()) {
		return false
	}
	q := "Installer Immich Wallpaper sur ce PC ?\n\n" +
		"• copie dans " + installDir() + "\n" +
		"• raccourci dans le menu Démarrer, lancement avec Windows\n" +
		"• désinstallable depuis Paramètres → Applications installées\n\n" +
		"« Non » = lancer sans installer."
	if isInstalled() {
		q = "Immich Wallpaper est déjà installé.\n\nRemplacer la version installée par celle-ci (" + appVersion + ") ?\nTa configuration est conservée.\n\n« Non » = lancer sans installer."
	}
	if !confirm(q) {
		return false
	}
	if err := install(); err != nil {
		msgBox("Immich Wallpaper", "Installation impossible : "+err.Error(), mbIconError)
		return false
	}
	if err := hiddenCmd(installedExe()).Start(); err != nil {
		msgBox("Immich Wallpaper", "Installé, mais impossible de le lancer : "+err.Error(), mbIconError)
	}
	return true
}

// --- désinstallation ---

// uninstall supprime tout : processus, démarrage auto, entrée « Applications », raccourci,
// données (config, token, journal) et les fichiers du programme.
func uninstall(quiet bool) {
	if !quiet && !confirm("Désinstaller Immich Wallpaper ?\n\nLa configuration (dont le token Immich) et le journal seront supprimés.\nLe fond d'écran actuel restera en place.") {
		return
	}
	stopRunningInstance()
	setAutostart(false)
	registry.DeleteKey(registry.CURRENT_USER, uninstallKey)
	os.Remove(startMenuShortcut())
	removeDesktopLink()
	os.RemoveAll(dataDir())

	// Un exe ne peut pas se supprimer lui-même pendant qu'il tourne : on confie la suppression
	// à un cmd caché qui attend notre sortie.
	self, _ := os.Executable()
	target := installDir()
	if !samePath(filepath.Dir(self), target) {
		os.RemoveAll(target)
		if !quiet {
			msgBox("Immich Wallpaper", "Immich Wallpaper a été désinstallé.", mbIconInfo)
		}
		return
	}
	// Message d'abord : le dossier ne peut être supprimé qu'une fois ce processus terminé.
	if !quiet {
		msgBox("Immich Wallpaper", "Immich Wallpaper a été désinstallé.", mbIconInfo)
	}
	cmd := exec.Command("cmd.exe")
	cmd.SysProcAttr = &syscall.SysProcAttr{
		HideWindow:    true,
		CreationFlags: 0x08000000,
		// Ligne de commande brute : cmd ne comprend pas l'échappement \" que ferait Go.
		CmdLine: fmt.Sprintf(`cmd.exe /c ping -n 4 127.0.0.1 >nul & rd /s /q "%s"`, target),
	}
	cmd.Start()
}
