//! The background pipeline that drives the status window.
//!
//! Ported from Nomad Launcher `core/src/lib.rs` (`pipeline_thread`,
//! `handle_error`, and the state/condvar helpers; MIT, see
//! `THIRD_PARTY_NOTICES.md`), using a blocking HTTP client instead of tokio.
//!
//! Flow: recover → check → (prompt) → download/verify/install → launch
//! `Cromite Portable.exe`, or show a final message when run with
//! `--no-launch`. Offline with an existing install launches it directly,
//! like Nomad.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::error::{Error, Result};
use crate::github::{self, VersionInfo};
use crate::ui::{ProgressState, StateHandle, StatusLines, WindowPhase};
use crate::updater::{self, Step};
use crate::version::{self, Installed};
use crate::{download, install};

/// The existing native launcher, which owns Cromite's privacy flags.
pub const LAUNCHER_EXE: &str = "Cromite Portable.exe";

pub struct Config {
    /// Portable folder containing `app/`, `data/` and the launcher.
    pub root: PathBuf,
    /// Launch Cromite when done (`false` with `--no-launch`).
    pub launch: bool,
    /// Accept the update prompt automatically (`--yes`).
    pub assume_yes: bool,
    /// Arguments passed through to Cromite (e.g. a URL).
    pub forwarded: Vec<String>,
}

enum PostErrorAction {
    Retry,
    Done,
}

pub fn pipeline_thread(cfg: &Config, state: &StateHandle, ctx: &egui::Context) {
    let app = cfg.root.join("app");
    loop {
        restart_pipeline(state, ctx);
        install::recover_staging(&app);
        let installed = version::installed(&app);
        set_installed_version(state, ctx, &installed);

        match run_once(cfg, &app, &installed, state, ctx) {
            Ok(()) => break,
            Err(e) => match handle_error(cfg, &installed, state, ctx, &e.to_string()) {
                PostErrorAction::Retry => {}
                PostErrorAction::Done => break,
            },
        }
    }
}

fn run_once(
    cfg: &Config,
    app: &Path,
    installed: &Installed,
    state: &StateHandle,
    ctx: &egui::Context,
) -> Result<()> {
    set_status(
        state,
        ctx,
        StatusLines::with_detail(
            "Checking for updates\u{2026}",
            "Fetching GitHub release metadata",
        ),
        ProgressState::Indeterminate,
    );

    let client = download::client()?;
    let latest = match github::fetch_latest(&client, github::LATEST_RELEASE_URL) {
        Ok(latest) => latest,
        Err(Error::Offline(_)) if *installed != Installed::Missing => {
            return finish(
                cfg,
                state,
                ctx,
                "Offline \u{2014} using the installed version",
            );
        }
        Err(e) => return Err(e),
    };
    set_build_date(state, ctx, &latest);

    if !version::needs_update(installed, &latest) {
        return finish(cfg, state, ctx, "Cromite is up to date");
    }

    let has_install = *installed != Installed::Missing;
    let decline = match (has_install, cfg.launch) {
        (false, _) => "Cancel",
        (true, true) => "Launch current",
        (true, false) => "Skip",
    };
    set_status(
        state,
        ctx,
        StatusLines::with_detail(
            "Update available",
            format!(
                "{:.0} MB download from github.com/uazo/cromite",
                mb(latest.size)
            ),
        ),
        ProgressState::Hidden,
    );
    let decision = if cfg.assume_yes {
        Some(true)
    } else {
        signal_update_prompt(state, ctx, &version::short_tag(&latest.tag), decline);
        wait_for_decision(state)
    };
    match decision {
        Some(true) => {}
        Some(false) if has_install => return finish(cfg, state, ctx, "Update skipped"),
        Some(false) | None => {
            signal_done(state, ctx);
            return Ok(());
        }
    }

    // Fail fast before spending 200 MB of bandwidth; the swap re-checks.
    install::ensure_not_running(app)?;

    let total_mb = mb(latest.size);
    updater::download_and_install(
        &client,
        app,
        &latest,
        |step| {
            let (lines, progress) = match step {
                Step::Downloading => (
                    StatusLines::with_detail("Downloading\u{2026}", "chrome-win.zip"),
                    ProgressState::Determinate(0.0),
                ),
                Step::Verifying => (
                    StatusLines::with_detail(
                        "Verifying\u{2026}",
                        "Checking the SHA-256 digest published on GitHub",
                    ),
                    ProgressState::Indeterminate,
                ),
                Step::Extracting => (
                    StatusLines::with_detail(
                        "Installing\u{2026}",
                        "Extracting into a staging folder",
                    ),
                    ProgressState::Indeterminate,
                ),
                Step::Installing => (
                    StatusLines::with_detail("Installing\u{2026}", "Swapping in the new build"),
                    ProgressState::Indeterminate,
                ),
            };
            set_status(state, ctx, lines, progress);
        },
        |done, total| {
            let total = total.unwrap_or(latest.size).max(1);
            #[allow(clippy::cast_precision_loss)]
            let fraction = done as f32 / total as f32;
            update_progress(
                state,
                ctx,
                fraction,
                format!("{:.1} / {total_mb:.1} MB", mb(done)),
            );
        },
    )?;

    set_installed_version(state, ctx, &Installed::Marked((&latest).into()));
    finish(
        cfg,
        state,
        ctx,
        &format!("Cromite {} installed", latest.chromium_version),
    )
}

/// Ends a successful run: launches Cromite (window closes) or, with
/// `--no-launch`, shows `outcome` with a Close button.
fn finish(cfg: &Config, state: &StateHandle, ctx: &egui::Context, outcome: &str) -> Result<()> {
    if cfg.launch {
        set_status(
            state,
            ctx,
            StatusLines::with_detail("Launching\u{2026}", outcome),
            ProgressState::Hidden,
        );
        launch(cfg)?;
        signal_done(state, ctx);
    } else {
        set_status(state, ctx, StatusLines::new(outcome), ProgressState::Hidden);
        set_phase(state, ctx, WindowPhase::Finished);
    }
    Ok(())
}

fn launch(cfg: &Config) -> Result<()> {
    let exe = cfg.root.join(LAUNCHER_EXE);
    if !exe.is_file() {
        return Err(Error::Io(std::io::Error::other(format!(
            "{LAUNCHER_EXE} was not found in {}",
            cfg.root.display()
        ))));
    }
    std::process::Command::new(exe)
        .args(&cfg.forwarded)
        .current_dir(&cfg.root)
        .spawn()?;
    Ok(())
}

fn handle_error(
    cfg: &Config,
    installed: &Installed,
    state: &StateHandle,
    ctx: &egui::Context,
    message: &str,
) -> PostErrorAction {
    let has_fallback = cfg.launch && *installed != Installed::Missing;
    show_error(state, ctx, message, has_fallback);

    loop {
        match wait_for_pipeline_action(state) {
            PipelineAction::Retry => return PostErrorAction::Retry,
            PipelineAction::Close => return PostErrorAction::Done,
            PipelineAction::LaunchAnyway => match launch(cfg) {
                Ok(()) => {
                    signal_done(state, ctx);
                    return PostErrorAction::Done;
                }
                Err(e) => show_error(state, ctx, &e.to_string(), false),
            },
        }
    }
}

#[allow(clippy::cast_precision_loss)] // display only
fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

// ── State mutation helpers (Nomad `lib.rs`) ──────────────────────────────────

fn with_state(
    state: &StateHandle,
    ctx: &egui::Context,
    f: impl FnOnce(&mut crate::ui::UpdaterState),
) {
    let (lock, cvar) = &**state;
    let mut guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard);
    cvar.notify_all();
    drop(guard);
    ctx.request_repaint();
}

fn restart_pipeline(state: &StateHandle, ctx: &egui::Context) {
    with_state(state, ctx, |s| {
        s.phase = WindowPhase::Running;
        s.view.status = StatusLines::new("Starting\u{2026}");
        s.view.progress = ProgressState::Indeterminate;
    });
}

fn set_status(
    state: &StateHandle,
    ctx: &egui::Context,
    status: StatusLines,
    progress: ProgressState,
) {
    with_state(state, ctx, |s| {
        s.view.status = status;
        s.view.progress = progress;
    });
}

fn update_progress(state: &StateHandle, ctx: &egui::Context, fraction: f32, detail: String) {
    with_state(state, ctx, |s| {
        s.view.progress = ProgressState::Determinate(fraction);
        s.view.status.secondary = detail;
    });
}

fn set_installed_version(state: &StateHandle, ctx: &egui::Context, installed: &Installed) {
    let (browser, engine) = match installed {
        Installed::Marked(v) => (
            Some(version::short_tag(&v.tag)),
            Some(v.chromium_version.clone()),
        ),
        Installed::Detected(v) => (Some(v.clone()), Some(v.clone())),
        Installed::Unknown => (Some("unknown version".to_owned()), None),
        Installed::Missing => (None, None),
    };
    with_state(state, ctx, |s| {
        s.view.browser_version = browser;
        s.view.engine_version = engine;
    });
}

fn set_build_date(state: &StateHandle, ctx: &egui::Context, latest: &VersionInfo) {
    let date = latest.published.clone();
    with_state(state, ctx, |s| s.view.build_date = date);
}

fn set_phase(state: &StateHandle, ctx: &egui::Context, phase: WindowPhase) {
    with_state(state, ctx, |s| s.phase = phase);
}

fn signal_done(state: &StateHandle, ctx: &egui::Context) {
    set_phase(state, ctx, WindowPhase::Done);
}

fn show_error(state: &StateHandle, ctx: &egui::Context, message: &str, has_fallback: bool) {
    set_phase(
        state,
        ctx,
        WindowPhase::Error {
            message: message.to_owned(),
            has_fallback,
        },
    );
}

fn signal_update_prompt(
    state: &StateHandle,
    ctx: &egui::Context,
    new_version: &str,
    decline: &str,
) {
    set_phase(
        state,
        ctx,
        WindowPhase::UpdatePrompt {
            new_version: new_version.to_owned(),
            decline_label: decline.to_owned(),
        },
    );
}

// ── Condvar waits (Nomad `lib.rs`) ───────────────────────────────────────────

/// Blocks until the user resolves the update prompt. `None` when the window
/// was closed first.
fn wait_for_decision(state: &StateHandle) -> Option<bool> {
    let (lock, cvar) = &**state;
    let mut guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        match &guard.phase {
            WindowPhase::UpdateDecided(download) => return Some(*download),
            WindowPhase::Done => return None,
            _ => {
                guard = cvar
                    .wait(guard)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

enum PipelineAction {
    Retry,
    LaunchAnyway,
    Close,
}

/// Blocks until the user clicks Retry, Launch anyway, or Close after an error.
fn wait_for_pipeline_action(state: &StateHandle) -> PipelineAction {
    let (lock, cvar) = &**state;
    let mut guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        match &guard.phase {
            WindowPhase::RetryRequested => return PipelineAction::Retry,
            WindowPhase::LaunchAnyway => return PipelineAction::LaunchAnyway,
            WindowPhase::Done => return PipelineAction::Close,
            _ => {
                guard = cvar
                    .wait(guard)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}
