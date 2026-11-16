package main

import (
	"errors"
	"fmt"
	"runtime"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Écran de verrouillage par appel direct à l'API WinRT Windows.System.UserProfile.LockScreen,
// dans le processus de l'appli : aucun PowerShell ni autre programme n'est lancé.
//
// Les interfaces sont appelées par leur vtable (comme IDesktopWallpaper dans
// monitors_windows.go). Indices d'après les métadonnées de Windows (Windows.winmd) :
// IInspectable occupe les emplacements 0 à 5 (IUnknown : 0-2).
var (
	combase                    = windows.NewLazySystemDLL("combase.dll")
	procRoInitialize           = combase.NewProc("RoInitialize")
	procRoUninitialize         = combase.NewProc("RoUninitialize")
	procRoGetActivationFactory = combase.NewProc("RoGetActivationFactory")
	procWindowsCreateString    = combase.NewProc("WindowsCreateString")
	procWindowsDeleteString    = combase.NewProc("WindowsDeleteString")

	iidStorageFileStatics, _ = windows.GUIDFromString("{5984C710-DAF2-43C8-8BB4-A4D3EACFD03F}")
	iidLockScreenStatics, _  = windows.GUIDFromString("{3EE9D3AD-B607-40AE-B426-7631D9821269}")
	iidAsyncInfo, _          = windows.GUIDFromString("{00000036-0000-0000-C000-000000000046}")
)

const (
	vtQueryInterface = 0
	vtRelease        = 2
	// IStorageFileStatics
	vtGetFileFromPathAsync = 6
	// ILockScreenStatics : OriginalImageFile (6), GetImageStream (7), SetImageFileAsync (8)
	vtSetImageFileAsync = 8
	// IAsyncOperation<T> et IAsyncAction : put_Completed (6), get_Completed (7), GetResults (8)
	vtGetResults = 8
	// IAsyncInfo : get_Id (6), get_Status (7), get_ErrorCode (8), Cancel (9), Close (10)
	vtAsyncStatus    = 7
	vtAsyncErrorCode = 8
	vtAsyncCancel    = 9

	asyncCompleted = 1
	asyncCanceled  = 2
	asyncError     = 3

	roInitMultithreaded = 1
	rpcEChangedMode     = 0x80010106
)

// errWinRTUnavailable : l'API n'a pas pu être appelée (ancienne version de Windows, composant
// absent…), par opposition à un refus de Windows lui-même (stratégie de l'entreprise, fichier).
var errWinRTUnavailable = errors.New("API WinRT indisponible")

type comObj struct{ p unsafe.Pointer }

// call appelle la méthode idx de la vtable. Les pointeurs Go passés en uintptr restent
// valides pendant l'appel (directive uintptrescapes, comme windows.Proc.Call).
//
//go:uintptrescapes
func (o comObj) call(idx int, args ...uintptr) int32 {
	vtbl := *(**[16]uintptr)(o.p)
	r, _, _ := syscall.SyscallN(vtbl[idx], append([]uintptr{uintptr(o.p)}, args...)...)
	return int32(r)
}

func (o comObj) release() {
	if o.p != nil {
		o.call(vtRelease)
	}
}

func hresultErr(what string, hr int32) error {
	msg := fmt.Sprintf("%s : HRESULT 0x%08X", what, uint32(hr))
	switch uint32(hr) {
	case 0x80070005:
		msg += " (accès refusé : l'écran de verrouillage est peut-être imposé par une stratégie de l'entreprise)"
	case 0x80070002, 0x80070003:
		msg += " (fichier introuvable)"
	}
	return errors.New(msg)
}

// hstring crée une chaîne WinRT ; à libérer avec WindowsDeleteString.
func hstring(s string) (uintptr, error) {
	u, err := windows.UTF16FromString(s)
	if err != nil {
		return 0, err
	}
	var h uintptr
	r, _, _ := procWindowsCreateString.Call(uintptr(unsafe.Pointer(&u[0])), uintptr(len(u)-1), uintptr(unsafe.Pointer(&h)))
	if int32(r) < 0 {
		return 0, hresultErr("WindowsCreateString", int32(r))
	}
	return h, nil
}

func deleteHstring(h uintptr) {
	if h != 0 {
		procWindowsDeleteString.Call(h)
	}
}

// activationFactory renvoie la fabrique (interface statique) d'une classe WinRT.
func activationFactory(class string, iid *windows.GUID) (comObj, error) {
	if err := procRoGetActivationFactory.Find(); err != nil {
		return comObj{}, fmt.Errorf("%w : %v", errWinRTUnavailable, err)
	}
	h, err := hstring(class)
	if err != nil {
		return comObj{}, err
	}
	defer deleteHstring(h)
	var f unsafe.Pointer
	r, _, _ := procRoGetActivationFactory.Call(h, uintptr(unsafe.Pointer(iid)), uintptr(unsafe.Pointer(&f)))
	if int32(r) < 0 || f == nil {
		return comObj{}, fmt.Errorf("%w : %v", errWinRTUnavailable, hresultErr("RoGetActivationFactory("+class+")", int32(r)))
	}
	return comObj{f}, nil
}

// await attend la fin d'une opération asynchrone WinRT (IAsyncOperation ou IAsyncAction)
// en interrogeant son état : pas de rappel à implémenter, l'appel est de toute façon bloquant.
func await(op comObj, what string, timeout time.Duration) error {
	var p unsafe.Pointer
	if hr := op.call(vtQueryInterface, uintptr(unsafe.Pointer(&iidAsyncInfo)), uintptr(unsafe.Pointer(&p))); hr < 0 || p == nil {
		return fmt.Errorf("%w : %v", errWinRTUnavailable, hresultErr(what+" (IAsyncInfo)", hr))
	}
	info := comObj{p}
	defer info.release()
	deadline := time.Now().Add(timeout)
	for wait := time.Millisecond; ; wait = min(wait*2, 50*time.Millisecond) {
		var st int32
		if hr := info.call(vtAsyncStatus, uintptr(unsafe.Pointer(&st))); hr < 0 {
			return hresultErr(what+" (état)", hr)
		}
		switch st {
		case asyncCompleted:
			return nil
		case asyncCanceled:
			return errors.New(what + " : annulé")
		case asyncError:
			var code int32
			info.call(vtAsyncErrorCode, uintptr(unsafe.Pointer(&code)))
			return hresultErr(what, code)
		}
		if time.Now().After(deadline) {
			info.call(vtAsyncCancel)
			return fmt.Errorf("%s : pas de réponse de Windows après %v", what, timeout)
		}
		time.Sleep(wait)
	}
}

// withWinRT exécute f sur un thread fixe où WinRT est initialisé.
func withWinRT(f func() error) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	if err := procRoInitialize.Find(); err != nil {
		return fmt.Errorf("%w : %v", errWinRTUnavailable, err)
	}
	r, _, _ := procRoInitialize.Call(roInitMultithreaded)
	switch {
	case int32(r) >= 0: // S_OK, ou S_FALSE si déjà initialisé : à équilibrer dans les deux cas
		defer procRoUninitialize.Call()
	case uint32(r) == rpcEChangedMode:
		// Thread déjà initialisé dans un autre mode : WinRT y est utilisable tel quel.
	default:
		return fmt.Errorf("%w : %v", errWinRTUnavailable, hresultErr("RoInitialize", int32(r)))
	}
	return f()
}

// storageFile ouvre un fichier en StorageFile (son interface par défaut, IStorageFile).
func storageFile(path string, timeout time.Duration) (comObj, error) {
	statics, err := activationFactory("Windows.Storage.StorageFile", &iidStorageFileStatics)
	if err != nil {
		return comObj{}, err
	}
	defer statics.release()
	h, err := hstring(path)
	if err != nil {
		return comObj{}, err
	}
	defer deleteHstring(h)
	var p unsafe.Pointer
	if hr := statics.call(vtGetFileFromPathAsync, h, uintptr(unsafe.Pointer(&p))); hr < 0 || p == nil {
		return comObj{}, hresultErr("StorageFile.GetFileFromPathAsync", hr)
	}
	op := comObj{p}
	defer op.release()
	if err := await(op, "StorageFile.GetFileFromPathAsync", timeout); err != nil {
		return comObj{}, err
	}
	var file unsafe.Pointer
	if hr := op.call(vtGetResults, uintptr(unsafe.Pointer(&file))); hr < 0 || file == nil {
		return comObj{}, hresultErr("StorageFile.GetFileFromPathAsync (résultat)", hr)
	}
	return comObj{file}, nil
}

// setLockScreenNative applique l'image à l'écran de verrouillage (même API que celle
// qu'appelait le script PowerShell, mais sans lancer de programme).
func setLockScreenNative(path string) error {
	const timeout = time.Minute
	return withWinRT(func() error {
		file, err := storageFile(path, timeout)
		if err != nil {
			return err
		}
		defer file.release()
		statics, err := activationFactory("Windows.System.UserProfile.LockScreen", &iidLockScreenStatics)
		if err != nil {
			return err
		}
		defer statics.release()
		var p unsafe.Pointer
		if hr := statics.call(vtSetImageFileAsync, uintptr(file.p), uintptr(unsafe.Pointer(&p))); hr < 0 || p == nil {
			return hresultErr("LockScreen.SetImageFileAsync", hr)
		}
		action := comObj{p}
		defer action.release()
		if err := await(action, "LockScreen.SetImageFileAsync", timeout); err != nil {
			return err
		}
		if hr := action.call(vtGetResults); hr < 0 {
			return hresultErr("LockScreen.SetImageFileAsync (résultat)", hr)
		}
		return nil
	})
}
