// Windowed application: no console window. Errors surface in the GUI, or in a
// message box if the window itself cannot open.
#![windows_subsystem = "windows"]

//! Cromite Portable updater.
//!
//! Opens Nomad Launcher's status window (ported egui UI), checks
//! `uazo/cromite` for a newer Windows build, offers it, installs it with
//! Nomad's verified stage-and-swap transaction, then launches Cromite through
//! `Cromite Portable.exe`.
//!
//! ```text
//! Cromite-Updater.exe [--no-launch] [--yes] [--root <DIR>] [args for Cromite...]
//! ```

mod download;
mod error;
mod extract;
mod github;
mod install;
mod pipeline;
mod taskbar;
mod ui;
mod updater;
mod version;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The Cromite icon from the repository root, used for the window logo,
/// title bar and taskbar (and embedded in the .exe by `build.rs`).
pub(crate) static ICON: &[u8] = include_bytes!("../../app.ico");

fn parse_args() -> std::result::Result<pipeline::Config, String> {
    let mut root: Option<PathBuf> = None;
    let mut launch = true;
    let mut assume_yes = false;
    let mut forwarded = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-launch" => launch = false,
            "--yes" => assume_yes = true,
            "--root" => {
                root = Some(PathBuf::from(
                    args.next().ok_or("--root needs a directory")?,
                ))
            }
            "--" => forwarded.extend(args.by_ref()),
            _ => forwarded.push(arg),
        }
    }
    let root = match root {
        Some(r) => r,
        None => {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .ok_or("cannot locate the updater's folder")?;
            find_portable_root(&exe_dir).ok_or_else(|| {
                format!(
                    "Could not find {} in {} or any folder above it.\n\n\
                     Place the updater in the Cromite Portable folder.",
                    pipeline::LAUNCHER_EXE,
                    exe_dir.display()
                )
            })?
        }
    };
    Ok(pipeline::Config {
        root,
        launch,
        assume_yes,
        forwarded,
    })
}

/// The portable folder is the nearest ancestor of the updater (itself
/// included) that holds `Cromite Portable.exe`. This covers both the shipped
/// layout (updater beside the launcher) and running the build output from
/// `updater/target/release/` inside the repository.
fn find_portable_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(pipeline::LAUNCHER_EXE).is_file())
        .map(Path::to_path_buf)
}

fn main() -> ExitCode {
    let cfg = match parse_args() {
        Ok(cfg) => cfg,
        Err(msg) => {
            show_msgbox(&msg);
            return ExitCode::FAILURE;
        }
    };

    let view = ui::LauncherView {
        display_name: "Cromite".to_owned(),
        id: "cromite".to_owned(),
        arch: "x64".to_owned(),
        browser_version: None,
        engine_name: "Chromium".to_owned(),
        engine_version: None,
        build_date: None,
        upstream_url: "https://github.com/uazo/cromite/releases".to_owned(),
        status: ui::StatusLines::new("Starting\u{2026}"),
        progress: ui::ProgressState::Indeterminate,
        icon_bytes: Some(ICON),
        accent: ui::theme::ACCENT,
    };

    match ui::show_window_driven(view, move |state, ctx| {
        pipeline::pipeline_thread(&cfg, &state, &ctx);
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            show_msgbox(&format!("Could not open the updater window:\n{e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn show_msgbox(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (text, title) = (wide(text), wide("Cromite Portable Updater"));
    // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

#[cfg(not(windows))]
fn show_msgbox(text: &str) {
    eprintln!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_launcher_folder_beside_the_updater() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(pipeline::LAUNCHER_EXE), b"x").unwrap();
        assert_eq!(find_portable_root(dir.path()).unwrap(), dir.path());
    }

    #[test]
    fn finds_the_repository_root_from_the_build_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(pipeline::LAUNCHER_EXE), b"x").unwrap();
        let release = dir.path().join("updater").join("target").join("release");
        std::fs::create_dir_all(&release).unwrap();
        assert_eq!(find_portable_root(&release).unwrap(), dir.path());
    }

    #[test]
    fn no_root_without_a_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        // Only meaningful if no ancestor of the temp dir holds the launcher.
        if !dir
            .path()
            .ancestors()
            .any(|d| d.join(pipeline::LAUNCHER_EXE).is_file())
        {
            assert!(find_portable_root(&nested).is_none());
        }
    }
}
