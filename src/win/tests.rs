//! Tests exécutés sur un vrai Windows.

use crate::win::app::retry_delay;
use crate::win::context::{blocked, connection_cost, fullscreen_busy, metered_connection, on_battery};
use crate::win::install::{same_path, uninstall_cmd_line, write_file_sync};
use crate::win::platform::*;
use std::os::windows::process::CommandExt;
use std::time::Duration;

#[test]
fn secret_round_trip() {
    let enc = protect_secret("clé-secrète-é").expect("chiffrement DPAPI indisponible");
    assert!(!enc.is_empty() && enc != "clé-secrète-é");
    assert_eq!(unprotect_secret(&enc).unwrap(), "clé-secrète-é");
    assert!(unprotect_secret("AAAA").is_err(), "valeur invalide déchiffrée");
    assert!(protect_secret("").is_none(), "une clé vide ne doit pas être chiffrée");
}

#[test]
fn system_programs_exist() {
    for name in ["cmd.exe", "taskkill.exe", "ping.exe"] {
        let p = system_exe(name);
        assert!(p.is_absolute() && p.exists(), "{}", p.display());
    }
}

/// La ligne de commande de désinstallation supprime bien le dossier, y compris avec des espaces.
#[test]
fn uninstall_command_line() {
    let target = crate::testutil::temp_dir("uninstall")
        .join("dossier avec espaces")
        .join("ImmichWallpaper");
    std::fs::create_dir_all(target.join("sous")).unwrap();
    std::fs::write(target.join("sous").join("f.txt"), "x").unwrap();
    let status = hidden_cmd(system_exe("cmd.exe"))
        .raw_arg(uninstall_cmd_line(&system_exe("ping.exe"), &target))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!target.exists(), "dossier non supprimé");
}

#[test]
fn retry_delays() {
    let want = [2u64, 2, 4, 8, 16, 30, 30];
    for (n, w) in want.iter().enumerate() {
        assert_eq!(retry_delay(n as u32), Duration::from_secs(w * 60), "après {n} échec(s)");
    }
}

/// Les fonctions de contexte doivent répondre sans erreur ni blocage.
#[test]
fn system_context() {
    match connection_cost() {
        Ok(c) => eprintln!("coût connexion={c:#x}"),
        // Service absent sur certaines éditions (Windows Server des machines de CI) :
        // l'appli considère alors la connexion comme non limitée.
        Err(e) => {
            eprintln!("coût de la connexion indisponible : {e}");
            assert!(!metered_connection());
        }
    }
    eprintln!(
        "batterie={} plein écran={} limitée={}",
        on_battery(),
        fullscreen_busy(),
        metered_connection()
    );
    let mut cfg = crate::config::Config::default();
    assert!(
        blocked(&cfg, true).is_none(),
        "rien ne doit bloquer avec la configuration par défaut"
    );
    let now = crate::util::now_local();
    // Plage d'une heure située à l'opposé de l'heure actuelle.
    let h = (now.hour + 12) % 24;
    cfg.active_from = format!("{h:02}:00");
    cfg.active_to = format!("{:02}:00", (h + 1) % 24);
    assert_eq!(blocked(&cfg, true), Some("hors plage horaire"));
    assert!(blocked(&cfg, false).is_none());
}

/// Version portable : données à côté de l'exe, entrée de démarrage distincte.
#[test]
fn portable_flavor() {
    let exe = std::env::current_exe().unwrap();
    let std_dir = data_dir_for(false);
    assert!(
        std_dir.parent() != exe.parent(),
        "version classique : données attendues dans le profil : {}",
        std_dir.display()
    );
    assert_eq!(data_dir_for(true), exe.parent().unwrap().join("ImmichWallpaper-data"));
    assert_eq!(autostart_value(false), RUN_VALUE);
    assert_ne!(autostart_value(true), RUN_VALUE);
    assert!(same_path(&exe, &std::path::PathBuf::from(exe.to_string_lossy().to_uppercase())));
}

#[test]
fn screen_and_monitors() {
    let sc = screen_for("fill");
    assert!(sc.w >= 320 && sc.h >= 240, "{sc:?}");
    let span = screen_for("span");
    assert!(span.w >= sc.w && span.h >= sc.h, "{span:?}");
    // L'énumération des écrans ne doit pas échouer (la liste peut être vide sans session graphique).
    for m in crate::win::monitors::list_monitors() {
        assert!(!m.id.is_empty() && m.screen.w > 0 && m.screen.h > 0);
    }
}

#[test]
fn file_sync() {
    let p = crate::testutil::temp_dir("sync").join("x.exe.new");
    write_file_sync(&p, b"MZ").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"MZ");
}

/// Parcours complet de la vraie appli (icône, planification, menu, événements) face au faux
/// serveur Immich, en mode vérification : rien n'est appliqué au bureau. L'exécutable à tester
/// est indiqué par la variable IW_APP_EXE ; sans elle, le test est ignoré.
#[test]
fn app_end_to_end() {
    let Some(exe) = std::env::var_os("IW_APP_EXE") else {
        eprintln!("IW_APP_EXE non défini : test ignoré");
        return;
    };
    let fake = crate::testutil::FakeImmich::start();
    let dir = crate::testutil::temp_dir("e2e");
    let mut cfg = crate::config::Config {
        server_url: fake.url.clone(),
        api_key: crate::testutil::KEY.into(),
        albums: vec![crate::testutil::ALB2.into()],
        show_date: true,
        auto_update: false,
        ..Default::default()
    };
    cfg.normalize().unwrap();
    crate::config::save_config(&dir.join("config.json"), &cfg).unwrap();
    let out = dir.join("result.txt");
    let status = std::process::Command::new(&exe)
        .arg("--selftest")
        .arg(&out)
        .arg("--selftest-dir")
        .arg(&dir)
        .status()
        .unwrap();
    let report = std::fs::read_to_string(&out).unwrap_or_default();
    let log = std::fs::read_to_string(dir.join("journal.txt")).unwrap_or_default();
    eprintln!("{report}");
    assert!(status.success() && report.starts_with("OK"), "{report}\n--- journal ---\n{log}");
    // La clé API a été relue (chiffrée par DPAPI) et le favori a bien été envoyé au serveur.
    assert!(
        fake.state.log.lock().unwrap().iter().any(|(m, _)| m == "PUT"),
        "aucune mise en favori reçue par le serveur"
    );
    assert!(dir.join("excluded.json").exists() && dir.join("recent.json").exists());

    // Lancée « cachée », comme la mise à jour des versions précédentes (Go : HideWindow) le
    // fait : la fenêtre de réglages doit quand même s'afficher.
    let hidden = crate::testutil::temp_dir("e2e_hidden");
    std::fs::copy(dir.join("config.json"), hidden.join("config.json")).unwrap();
    let out = hidden.join("result.txt");
    let code = run_hidden(
        &exe,
        &["--selftest", &out.to_string_lossy(), "--selftest-dir", &hidden.to_string_lossy()],
    );
    let report = std::fs::read_to_string(&out).unwrap_or_default();
    let log = std::fs::read_to_string(hidden.join("journal.txt")).unwrap_or_default();
    eprintln!("lancement caché : {report}");
    assert!(code == 0 && report.starts_with("OK"), "{report}\n--- journal ---\n{log}");
}

/// Lance un programme avec STARTF_USESHOWWINDOW + SW_HIDE (ce que fait HideWindow en Go) et
/// renvoie son code de sortie.
fn run_hidden(exe: &std::ffi::OsStr, args: &[&str]) -> u32 {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, WaitForSingleObject, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
        STARTUPINFOW,
    };
    let mut line = format!("\"{}\"", exe.to_string_lossy());
    for a in args {
        line += &format!(" \"{a}\"");
    }
    let mut wline = wide(&line);
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: 0, // SW_HIDE
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            None,
            Some(PWSTR(wline.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &si,
            &mut pi,
        )
        .expect("lancement de l'appli");
        WaitForSingleObject(pi.hProcess, 120_000);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
        let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
        code
    }
}

/// TLS par Schannel face à un vrai serveur : vérification normale, puis épinglage du certificat.
/// Demande un accès à Internet : lancé seulement avec IW_NET_TESTS=1.
#[test]
fn tls_and_pinning() {
    if std::env::var_os("IW_NET_TESTS").is_none() {
        eprintln!("IW_NET_TESTS non défini : test ignoré");
        return;
    }
    use std::sync::{Arc, Mutex};
    let get = |pin: Option<Arc<Mutex<String>>>| {
        crate::immich::http_agent(pin, 5)
            .get("https://api.github.com/zen")
            .call()
            .map(|r| r.status())
            .map_err(|e| e.to_string())
    };
    assert_eq!(get(None), Ok(200), "vérification normale du certificat");
    // Sans empreinte connue : le certificat présenté est adopté.
    let pin = Arc::new(Mutex::new(String::new()));
    assert_eq!(get(Some(pin.clone())), Ok(200));
    assert_eq!(pin.lock().unwrap().len(), 64, "empreinte non relevée");
    // Même empreinte : accepté ; autre empreinte : refusé avant tout envoi.
    assert_eq!(get(Some(pin.clone())), Ok(200));
    let wrong = Arc::new(Mutex::new("ab".repeat(32)));
    let e = get(Some(wrong)).unwrap_err();
    assert!(e.contains("certificat"), "{e}");
    // Mise à jour : la release en ligne est lue et ses adresses sont acceptées.
    let u = crate::update::check_update("0.0.1", &crate::update::asset_name("x86_64", false));
    assert!(matches!(u, Ok(Some(_))), "{u:?}");
}
