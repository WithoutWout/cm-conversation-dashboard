//! Self-update, including the Windows **portable** build.
//!
//! `tauri-plugin-updater` can only install through an installer on Windows
//! (NSIS or MSI). The portable `.exe` is the primary way this app is
//! distributed — an installer needs privileges some users' IT policy withholds
//! — so it needs an install path of its own.
//!
//! ## The rename trick
//!
//! Windows will not let you write to or delete a running `.exe`: the loader
//! holds the image open. It *will* let you **rename** it, because the image is
//! opened with `FILE_SHARE_DELETE` and a rename only rewrites a directory
//! entry, never the file data. That single fact is what makes an in-place
//! portable update possible, and it is what Chrome, Firefox and Squirrel all
//! rely on. The sequence in [`apply_portable_zip`] is:
//!
//! ```text
//!   write   <dir>/CAIDashboard.exe.new     (verified bytes, same directory)
//!   rename  CAIDashboard.exe      -> .old  (allowed while running)
//!   rename  CAIDashboard.exe.new  -> CAIDashboard.exe
//!   spawn CAIDashboard.exe; exit
//!   next launch deletes *.exe.old
//! ```
//!
//! Everything happens in the app's own directory, so both renames are
//! same-volume and therefore atomic, and the first one is undoable: if the
//! second rename fails we put `.old` back and the user is exactly where they
//! started. Nothing is ever deleted until a *later* launch has proved the new
//! binary runs, which is also what makes a bad release recoverable by hand.
//!
//! ## Why the bytes are already trusted here
//!
//! `apply_portable_zip` executes what it is given on the next launch, so it
//! must never be handed unverified input. It isn't: the caller gets its bytes
//! from `tauri_plugin_updater::Update::download`, which verifies the minisign
//! signature against the `pubkey` in `tauri.conf.json` *before* returning
//! them. We deliberately reuse the plugin for check/download/verify and
//! override only the install step — rolling our own signature check for a
//! payload we are about to make the application would be the wrong place to
//! save a dependency.

use std::path::{Path, PathBuf};

/// How this copy of the app can update itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// A bare Windows `.exe`, updated by [`apply_portable_zip`].
    Portable,
    /// Installed by NSIS, or a macOS `.app` — the plugin's own installer path.
    Managed,
}

impl InstallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallKind::Portable => "portable",
            InstallKind::Managed => "managed",
        }
    }
}

/// The updater target key looked up in `latest.json`.
///
/// `None` leaves the plugin to derive the usual `{os}-{arch}` /
/// `{os}-{arch}-{installer}` key. The portable build asks for a key of its own
/// because it needs a different artifact — the plain zip, not the NSIS setup —
/// from the same release.
pub fn updater_target() -> Option<String> {
    match install_kind() {
        InstallKind::Portable => Some("windows-portable".to_string()),
        InstallKind::Managed => None,
    }
}

/// Whether this copy is a portable `.exe` or a managed install.
///
/// Decided by the presence of `uninstall.exe` beside the binary, which the
/// Tauri NSIS template always writes (`WriteUninstaller "$INSTDIR\uninstall.exe"`)
/// and which a portable zip — one file — never contains. This is a property of
/// the directory we would actually be modifying, which the registry is not:
/// someone who copies an installed `CAIDashboard.exe` onto a USB stick is
/// portable from that point on, and this check says so.
///
/// It is deliberately biased toward `Portable`. Misreading a portable copy as
/// managed would run an installer it cannot use; misreading a managed install
/// as portable just swaps the binary in place, which works — the install stays
/// runnable, only its uninstall entry reports a stale version.
pub fn install_kind() -> InstallKind {
    if !cfg!(windows) {
        return InstallKind::Managed;
    }
    match current_exe_dir() {
        Ok(dir) if dir.join("uninstall.exe").is_file() => InstallKind::Managed,
        Ok(_) => InstallKind::Portable,
        // Without a resolvable exe path we cannot swap anything anyway; let the
        // plugin's own path report the failure.
        Err(_) => InstallKind::Managed,
    }
}

/// Directory holding the running executable.
pub fn current_exe_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate the application: {e}"))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "The application has no parent directory.".to_string())
}

/// Whether the app's own directory can be written to.
///
/// A portable copy can sit anywhere: a read-only network share, a locked-down
/// `Program Files`, a folder under Controlled Folder Access. Probing with a
/// real file is the only honest answer — permission bits alone do not account
/// for any of those. Checked *before* an update is offered, so the user never
/// gets a button that fails halfway through.
pub fn exe_dir_writable() -> bool {
    let Ok(dir) = current_exe_dir() else {
        return false;
    };
    let probe = dir.join(".cai-update-probe");
    let ok = std::fs::write(&probe, b"probe").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Largest payload accepted out of the update zip.
///
/// The archive is signature-verified before it reaches us, so this is a guard
/// against a mistake on our side rather than an attacker — a wrong asset
/// wired into `latest.json` should fail loudly instead of being decompressed
/// into memory until the process dies.
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;

/// Pull the single `.exe` out of a verified portable zip.
fn exe_from_zip(zip_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("Update archive is unreadable: {e}"))?;

    let index = (0..archive.len())
        .find(|&i| {
            archive
                .by_index_raw(i)
                .ok()
                .and_then(|f| {
                    f.enclosed_name()
                        .map(|n| n.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")))
                })
                .unwrap_or(false)
        })
        .ok_or_else(|| "Update archive contains no .exe.".to_string())?;

    let mut entry = archive
        .by_index(index)
        .map_err(|e| format!("Cannot read the update archive: {e}"))?;
    if entry.size() > MAX_UNPACKED_BYTES {
        return Err(format!(
            "Update executable is implausibly large ({} bytes).",
            entry.size()
        ));
    }

    let mut out = Vec::with_capacity(entry.size() as usize);
    std::io::copy(&mut entry, &mut out).map_err(|e| format!("Cannot unpack the update: {e}"))?;

    // A truncated or wrong asset would otherwise only be discovered by the user,
    // after we had already renamed the working binary out of the way.
    if out.len() < 2 || &out[..2] != b"MZ" {
        return Err("Update payload is not a Windows executable.".to_string());
    }
    Ok(out)
}

/// A `.old` path not currently in use.
///
/// The plain `.old` name is reused whenever possible so backups do not
/// accumulate, but a previous update's backup can still be locked by a process
/// that has not fully exited. Falling back to a numbered name keeps the update
/// working instead of failing on a leftover.
fn free_backup_path(dir: &Path, file_name: &str) -> Result<PathBuf, String> {
    let base = dir.join(format!("{file_name}.old"));
    if !base.exists() || std::fs::remove_file(&base).is_ok() {
        return Ok(base);
    }
    for n in 1..=99 {
        let candidate = dir.join(format!("{file_name}.old.{n}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot free a backup filename next to the application.".to_string())
}

/// Replace the running portable executable with the one in `zip_bytes`.
///
/// Returns the path to relaunch. The caller is responsible for spawning it and
/// exiting — see [`relaunch`].
///
/// `zip_bytes` **must** already be signature-verified; see the module docs.
pub fn apply_portable_zip(zip_bytes: &[u8]) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate the application: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "The application has no parent directory.".to_string())?
        .to_path_buf();
    let file_name = exe
        .file_name()
        .ok_or_else(|| "The application has no filename.".to_string())?
        .to_string_lossy()
        .into_owned();

    let payload = exe_from_zip(zip_bytes)?;

    // Same directory as the target, so both renames below stay on one volume
    // and are therefore atomic.
    let staged = dir.join(format!("{file_name}.new"));
    let _ = std::fs::remove_file(&staged);
    std::fs::write(&staged, &payload).map_err(|e| {
        format!("Cannot write the update next to the application ({}): {e}", dir.display())
    })?;

    let backup = free_backup_path(&dir, &file_name).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })?;

    // Allowed while running — this is the whole trick. See the module docs.
    std::fs::rename(&exe, &backup).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("Cannot move the current version aside: {e}")
    })?;

    if let Err(e) = std::fs::rename(&staged, &exe) {
        // Put it back. Failing here without this leaves no executable at all.
        let restored = std::fs::rename(&backup, &exe).is_ok();
        let _ = std::fs::remove_file(&staged);
        return Err(if restored {
            format!("Could not install the update, so nothing was changed: {e}")
        } else {
            format!(
                "Could not install the update and the previous version could not be restored. \
                 It is saved as {}. Rename it back to {file_name} to recover. ({e})",
                backup.display()
            )
        });
    }

    Ok(exe)
}

/// Start the updated executable and quit this one.
///
/// Returns `Err` only when the new binary could not be *started* — by then the
/// swap has already succeeded, so the caller must say "installed, but open it
/// yourself" rather than "the update failed". Reporting that as a plain failure
/// would send the user looking for a problem that no longer exists.
pub fn relaunch(app: &tauri::AppHandle, exe: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map_err(|e| {
        log::error!(target: "update", "could not relaunch {}: {e}", exe.display());
        format!(
            "The update was installed, but the app could not restart itself: {e}. \
             Close this window and open {} again.",
            exe.display()
        )
    })?;
    app.exit(0);
    Ok(())
}

/// Delete backups left by a previous update.
///
/// Runs on a background thread with retries: this process was spawned by the
/// one it is trying to clean up after, and Windows only releases the image once
/// that process has fully exited. Failures are ignored — the next launch tries
/// again, so the file cannot outlive two updates, and a locked backup is never
/// worth blocking startup for.
pub fn cleanup_stale_backups() {
    let Ok(dir) = current_exe_dir() else {
        return;
    };
    std::thread::spawn(move || {
        for attempt in 0..10 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return;
            };
            let mut remaining = false;
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                // Deliberately not a bare `*.old`: that is a common enough
                // suffix for a user's own files to be worth not touching.
                if !is_backup_name(&name) {
                    continue;
                }
                if std::fs::remove_file(entry.path()).is_err() {
                    remaining = true;
                }
            }
            if !remaining {
                return;
            }
        }
    });
}

/// `CAIDashboard.exe.old`, or `CAIDashboard.exe.old.3` from [`free_backup_path`].
fn is_backup_name(name: &str) -> bool {
    let Some(rest) = name.split_once(".exe.old").map(|(_, rest)| rest) else {
        return false;
    };
    rest.is_empty() || rest.strip_prefix('.').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_are_recognised_without_catching_user_files() {
        assert!(is_backup_name("CAIDashboard.exe.old"));
        assert!(is_backup_name("CAIDashboard.exe.old.1"));
        assert!(is_backup_name("CAIDashboard.exe.old.42"));
        // A user's own `.old` files share the directory with a portable app.
        assert!(!is_backup_name("notes.old"));
        assert!(!is_backup_name("CAIDashboard.exe"));
        assert!(!is_backup_name("settings.json.old"));
        assert!(!is_backup_name("CAIDashboard.exe.old.keep"));
        assert!(!is_backup_name("CAIDashboard.exe.old."));
    }

    /// The guard that stops a wrong or truncated asset from becoming the app.
    #[test]
    fn a_zip_without_a_pe_executable_is_rejected() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("CAIDashboard.exe", opts).unwrap();
            std::io::Write::write_all(&mut w, b"not an executable").unwrap();
            w.finish().unwrap();
        }
        let err = exe_from_zip(&buf).unwrap_err();
        assert!(err.contains("not a Windows executable"), "{err}");
    }

    #[test]
    fn the_exe_is_found_whatever_else_the_archive_holds() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("README.txt", opts).unwrap();
            std::io::Write::write_all(&mut w, b"hello").unwrap();
            w.start_file("CAIDashboard.exe", opts).unwrap();
            std::io::Write::write_all(&mut w, b"MZ\x90\x00payload").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(exe_from_zip(&buf).unwrap(), b"MZ\x90\x00payload");
    }

    #[test]
    fn an_archive_with_no_exe_is_an_error() {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("README.txt", opts).unwrap();
            std::io::Write::write_all(&mut w, b"hello").unwrap();
            w.finish().unwrap();
        }
        assert!(exe_from_zip(&buf).unwrap_err().contains("no .exe"));
    }

    /// The rename dance, exercised end to end against a stand-in "executable".
    /// Not `apply_portable_zip` itself — that reads `current_exe()` — but the
    /// same two renames plus the rollback path, which is where the risk is.
    #[test]
    fn a_failed_second_rename_leaves_the_original_in_place() {
        let dir = std::env::temp_dir().join(format!(
            "cai-update-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("App.exe");
        std::fs::write(&exe, b"MZ original").unwrap();

        let backup = free_backup_path(&dir, "App.exe").unwrap();
        std::fs::rename(&exe, &backup).unwrap();
        assert!(!exe.exists(), "original moved aside");

        // Simulate the staged file being gone, i.e. the second rename failing.
        assert!(std::fs::rename(dir.join("App.exe.new"), &exe).is_err());
        std::fs::rename(&backup, &exe).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ original");
        std::fs::remove_dir_all(&dir).ok();
    }
}
