//! Cromite Portable updater — status window.
//!
//! Ported from Nomad Launcher `core/src/ui/mod.rs` (MIT, see
//! `THIRD_PARTY_NOTICES.md`). Changes from Nomad: the Nomad brand lockup is
//! replaced by a Cromite footer, the update prompt's decline label is
//! configurable, and a [`WindowPhase::Finished`] state exists for update-only
//! runs that end without launching the browser.
//!
//! # Architecture (pipeline ↔ window)
//!
//! The window runs on the main thread (required by eframe). The pipeline runs
//! on a dedicated background OS thread. They share a
//! `Arc<(Mutex<`[`UpdaterState`]`>, Condvar)>` ([`StateHandle`]):
//!
//! - The pipeline writes [`UpdaterState`] and calls [`egui::Context::request_repaint`].
//! - The window reads the state each frame and renders accordingly.
//! - User decisions (update prompt, retry, launch-anyway) are returned to the
//!   pipeline via [`WindowPhase`] and the [`Condvar`].

pub mod identity;
pub mod theme;

use std::sync::{Arc, Condvar, Mutex};

use eframe::egui;

/// Atkinson Hyperlegible (SIL OFL) — the UI typeface, embedded so eframe can
/// be built without `default_fonts`. License: `assets/OFL.txt`.
const UI_FONT: &[u8] = include_bytes!("../../assets/AtkinsonHyperlegible-Regular.ttf");

// ── View model types ──────────────────────────────────────────────────────────

/// Progress-bar state for the identity card.
#[derive(Debug, Clone, PartialEq)]
pub enum ProgressState {
    /// Animated sweep — used during check / verify / extract.
    Indeterminate,
    /// Determinate fraction (0.0 – 1.0) — used during download.
    Determinate(f32),
    /// No bar drawn.
    Hidden,
}

/// Status text shown in the identity card's status block.
#[derive(Debug, Clone)]
pub struct StatusLines {
    /// Primary status (14 px) — e.g. `"Checking for updates…"`.
    pub primary: String,
    /// Secondary detail (11 px) — finer sub-step; may be empty.
    pub secondary: String,
}

impl StatusLines {
    /// Creates a status with the given `primary` text and an empty secondary.
    #[must_use]
    pub fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            secondary: String::new(),
        }
    }

    /// Creates a status with both lines.
    #[must_use]
    pub fn with_detail(primary: impl Into<String>, secondary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            secondary: secondary.into(),
        }
    }
}

/// Complete view model for the status window.
#[derive(Debug, Clone)]
pub struct LauncherView {
    /// Display name, e.g. `"Cromite"`.
    pub display_name: String,
    /// Runtime id used in the *Name* detail row, e.g. `"cromite"`.
    pub id: String,
    /// Architecture string, e.g. `"x64"`.
    pub arch: String,
    /// Installed browser version — `None` when nothing is installed.
    pub browser_version: Option<String>,
    /// Engine display name, e.g. `"Chromium"`.
    pub engine_name: String,
    /// Engine version — `None` when unknown.
    pub engine_version: Option<String>,
    /// Release date of the latest build; `None` until the check resolves.
    pub build_date: Option<String>,
    /// URL for the *Open upstream release page* footer link.
    pub upstream_url: String,
    /// Status lines shown in the identity card's status block.
    pub status: StatusLines,
    /// Progress bar state.
    pub progress: ProgressState,
    /// Raw `.ico` bytes for the logo, title bar and taskbar icon.
    pub icon_bytes: Option<&'static [u8]>,
    /// Accent colour for the progress bar, links and buttons.
    pub accent: egui::Color32,
}

impl LauncherView {
    /// `{browser_version} — {engine} {engine_version} (Portable)`, with the
    /// engine segment omitted when the two versions are equal.
    #[must_use]
    pub fn version_subtitle(&self) -> String {
        match (&self.browser_version, &self.engine_version) {
            (Some(bv), Some(ev)) if bv != ev => {
                format!("{bv} \u{2014} {} {ev} (Portable)", self.engine_name)
            }
            (Some(bv), _) => format!("{bv} (Portable)"),
            (None, _) => "Not installed (Portable)".to_owned(),
        }
    }
}

// ── Shared state (pipeline ↔ window) ─────────────────────────────────────────

/// Control-flow phase: what the pipeline is doing / what the window should show.
#[derive(Debug, Clone)]
pub enum WindowPhase {
    /// Pipeline running normally — render status lines and progress bar.
    Running,
    /// Update detected; waiting for the user to decide.
    UpdatePrompt {
        /// Version string of the available update.
        new_version: String,
        /// Label of the decline button (`"Launch current"`, `"Skip"`, …).
        decline_label: String,
    },
    /// Set by the window after the user decides: `true` = download, `false` = skip.
    UpdateDecided(bool),
    /// Browser spawned — window should close itself.
    Done,
    /// The run ended without launching (`--no-launch`); the status line holds
    /// the outcome and a Close button is shown.
    Finished,
    /// Pipeline failed — show error UI with action buttons.
    Error {
        /// Human-readable error description.
        message: String,
        /// Whether a usable install exists that the user could launch anyway.
        has_fallback: bool,
    },
    /// Set by the window when the user clicks *Retry*.
    RetryRequested,
    /// Set by the window when the user clicks *Launch anyway*.
    LaunchAnyway,
}

/// Shared state between the eframe window and the pipeline thread.
#[derive(Debug, Clone)]
pub struct UpdaterState {
    /// The current view model rendered each frame.
    pub view: LauncherView,
    /// The current control-flow phase.
    pub phase: WindowPhase,
}

/// Thread-safe handle shared between the pipeline thread and the eframe window.
pub type StateHandle = Arc<(Mutex<UpdaterState>, Condvar)>;

// ── eframe App ────────────────────────────────────────────────────────────────

struct UpdaterWindow {
    state: StateHandle,
    /// Browser-logo texture, lazily decoded once from [`LauncherView::icon_bytes`].
    logo: Option<egui::TextureHandle>,
    /// `ITaskbarList3` wrapper; drives taskbar-button progress on Windows.
    taskbar: crate::taskbar::TaskbarProgress,
    /// Window handle; populated on the first frame.
    hwnd: Option<crate::taskbar::Hwnd>,
}

impl eframe::App for UpdaterWindow {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Snapshot state without holding the lock during rendering.
        let (phase, view) = {
            let (lock, _) = &*self.state;
            let guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (guard.phase.clone(), guard.view.clone())
        };

        if self.hwnd.is_none() {
            self.hwnd = crate::taskbar::acquire_hwnd();
        }
        if let Some(hwnd) = self.hwnd {
            let is_error = matches!(phase, WindowPhase::Error { .. });
            self.taskbar.apply(hwnd, &view.progress, is_error);
        }

        if matches!(phase, WindowPhase::Done) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.logo.is_none() {
            if let Some(bytes) = view.icon_bytes {
                if let Some((rgba, w, h)) = decode_ico_rgba(bytes, 64) {
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    self.logo =
                        Some(ctx.load_texture("browser-logo", image, egui::TextureOptions::LINEAR));
                }
            }
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ctx, |ui| {
                ui.set_min_width(340.0);

                identity::identity_card(ui, &view, self.logo.as_ref());
                ui.add_space(theme::space::MD);
                runtime_details_card(ui, &view);
                ui.add_space(theme::space::MD);

                match &phase {
                    WindowPhase::UpdatePrompt {
                        new_version,
                        decline_label,
                    } => {
                        self.render_update_prompt(ui, new_version, decline_label);
                    }
                    WindowPhase::Error {
                        message,
                        has_fallback,
                    } => {
                        self.render_error(ui, ctx, message, *has_fallback);
                    }
                    WindowPhase::Finished => render_finished(ui, ctx),
                    _ => footer(ui, &view.upstream_url, view.accent),
                }
            });
    }
}

impl UpdaterWindow {
    /// Renders the *Update available* prompt with Update / decline buttons.
    fn render_update_prompt(&self, ui: &mut egui::Ui, new_version: &str, decline_label: &str) {
        ui.add_space(theme::space::SM);
        ui.label(
            egui::RichText::new(format!("Version {new_version} is available."))
                .size(theme::text::BODY)
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(theme::space::MD);
        ui.horizontal(|ui| {
            if ui.button("Update").clicked() {
                self.signal_phase(WindowPhase::UpdateDecided(true));
            }
            if ui.button(decline_label).clicked() {
                self.signal_phase(WindowPhase::UpdateDecided(false));
            }
        });
    }

    /// Renders the error message with Retry / Launch-anyway / Close buttons.
    fn render_error(
        &self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        message: &str,
        has_fallback: bool,
    ) {
        ui.add_space(theme::space::SM);
        ui.label(
            egui::RichText::new(message)
                .size(theme::text::BODY)
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(theme::space::SM);
        ui.horizontal(|ui| {
            if ui.button("Retry").clicked() {
                self.signal_phase(WindowPhase::RetryRequested);
            }
            if has_fallback && ui.button("Launch anyway").clicked() {
                self.signal_phase(WindowPhase::LaunchAnyway);
            }
            if ui.button("Close").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Locks the state, sets the phase, and notifies the pipeline condvar.
    fn signal_phase(&self, phase: WindowPhase) {
        let (lock, cvar) = &*self.state;
        let mut guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.phase = phase;
        cvar.notify_all();
    }
}

// ── Helper widgets ────────────────────────────────────────────────────────────

/// Renders the Close button that ends an update-only run.
fn render_finished(ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.add_space(theme::space::SM);
    if ui.button("Close").clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Renders the runtime details card (`RUNTIME DETAILS` eyebrow + four rows).
fn runtime_details_card(ui: &mut egui::Ui, view: &LauncherView) {
    let frame = egui::Frame::NONE
        .fill(theme::CARD)
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_CARD))
        .inner_margin(egui::Margin::same(12));

    frame.show(ui, |ui| {
        ui.label(
            egui::RichText::new("RUNTIME DETAILS")
                .size(theme::text::EYEBROW)
                .color(theme::EYEBROW),
        );

        ui.add_space(theme::space::MD);

        let name_val = format!("{} {}", view.id, view.arch);
        let version_key = format!("{} version", view.display_name);
        let version_val = view.browser_version.as_deref().unwrap_or("\u{2014}");
        let date_val = view.build_date.as_deref().unwrap_or("\u{2014}");

        detail_row(ui, "Name", &name_val);
        detail_row(ui, "Bundle mode", "Self-updating portable");
        detail_row(ui, &version_key, version_val);
        detail_row(ui, "Latest release", date_val);
    });
}

/// Renders a single `key  ···  value` row in the runtime details card.
fn detail_row(ui: &mut egui::Ui, key: &str, val: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(key)
                .size(theme::text::BODY)
                .color(theme::TEXT_SECONDARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(val)
                    .size(theme::text::BODY)
                    .color(theme::TEXT_PRIMARY),
            );
        });
    });
    ui.add_space(theme::space::SM);
}

/// Renders the footer: the upstream-release hyperlink (left) and the
/// `CROMITE PORTABLE` wordmark (right).
fn footer(ui: &mut egui::Ui, url: &str, accent: egui::Color32) {
    ui.add_space(theme::space::XS);
    ui.horizontal(|ui| {
        ui.hyperlink_to(
            egui::RichText::new("Open upstream release page")
                .size(theme::text::BODY)
                .color(accent),
            url,
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new("CROMITE PORTABLE")
                    .size(theme::text::BODY)
                    .color(accent)
                    .strong(),
            );
        });
    });
    ui.add_space(theme::space::XS);
}

// ── Icon helpers ─────────────────────────────────────────────────────────────

/// Decodes the `.ico` frame whose width is closest to `target_px` into
/// `(rgba, width, height)` straight-alpha RGBA8 pixels. Handles both BMP/DIB
/// and PNG-compressed frames.
fn decode_ico_rgba(bytes: &[u8], target_px: u32) -> Option<(Vec<u8>, u32, u32)> {
    let dir = ico::IconDir::read(std::io::Cursor::new(bytes)).ok()?;
    let entry = dir
        .entries()
        .iter()
        .min_by_key(|e| u64::from(e.width()).abs_diff(u64::from(target_px)))?;
    let image = entry.decode().ok()?;
    Some((image.rgba_data().to_vec(), image.width(), image.height()))
}

/// Decodes a `.ico` file into [`egui::viewport::IconData`] for the window /
/// taskbar icon.
fn parse_ico_icon(bytes: &[u8]) -> Option<egui::viewport::IconData> {
    let (rgba, width, height) = decode_ico_rgba(bytes, 64)?;
    Some(egui::viewport::IconData {
        rgba,
        width,
        height,
    })
}

/// A 32×32 solid `accent` icon used when no icon bytes are provided.
fn make_placeholder_icon(accent: egui::Color32) -> egui::viewport::IconData {
    const SIZE: u32 = 32;
    let (r, g, b) = (accent.r(), accent.g(), accent.b());
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[r, g, b, 0xFF]);
    }
    egui::viewport::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}

// ── Fonts & visuals ───────────────────────────────────────────────────────────

/// Installs [`UI_FONT`] as the only proportional and monospace family.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "atkinson".to_owned(),
        Arc::new(egui::FontData::from_static(UI_FONT)),
    );
    let family = vec!["atkinson".to_owned()];
    fonts
        .families
        .insert(egui::FontFamily::Proportional, family.clone());
    fonts.families.insert(egui::FontFamily::Monospace, family);
    ctx.set_fonts(fonts);
}

/// Applies the Nomad dark visuals, deriving widget styling from the palette
/// and `accent` so stock egui widgets match the bespoke cards.
fn setup_visuals(ctx: &egui::Context, accent: egui::Color32) {
    let mut vis = egui::Visuals::dark();
    vis.panel_fill = theme::BG;
    vis.window_fill = theme::BG;
    vis.override_text_color = Some(theme::TEXT_PRIMARY);
    vis.hyperlink_color = accent;

    let radius = egui::CornerRadius::same(theme::RADIUS_CARD);
    let w = &mut vis.widgets;
    w.inactive.bg_fill = theme::CARD;
    w.inactive.weak_bg_fill = theme::CARD;
    w.inactive.bg_stroke = egui::Stroke::new(1.0_f32, theme::BORDER);
    w.inactive.fg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_PRIMARY);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = theme::BORDER;
    w.hovered.weak_bg_fill = theme::BORDER;
    w.hovered.bg_stroke = egui::Stroke::new(1.0_f32, accent);
    w.hovered.fg_stroke = egui::Stroke::new(1.0_f32, theme::TEXT_PRIMARY);
    w.hovered.corner_radius = radius;
    w.active.bg_fill = accent;
    w.active.weak_bg_fill = accent;
    w.active.bg_stroke = egui::Stroke::new(1.0_f32, accent);
    w.active.fg_stroke = egui::Stroke::new(1.0_f32, theme::ON_ACCENT);
    w.active.corner_radius = radius;

    vis.selection.bg_fill = accent.gamma_multiply(0.4);
    vis.selection.stroke = egui::Stroke::new(1.0_f32, accent);
    ctx.set_visuals(vis);
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Opens the status window, starts `start_pipeline` on a background thread,
/// and blocks until the window is closed.
///
/// The pipeline drives the window by setting [`UpdaterState::phase`]:
/// [`WindowPhase::Done`] closes it, [`WindowPhase::Error`] shows error
/// buttons, [`WindowPhase::UpdatePrompt`] shows the update prompt, and
/// [`WindowPhase::Finished`] shows a Close button.
pub fn show_window_driven<F>(view: LauncherView, start_pipeline: F) -> Result<(), eframe::Error>
where
    F: FnOnce(StateHandle, egui::Context) + Send + 'static,
{
    let title = format!("{} Portable \u{2014} Updater", view.display_name);
    let accent = view.accent;
    let icon = view
        .icon_bytes
        .and_then(parse_ico_icon)
        .unwrap_or_else(|| make_placeholder_icon(accent));

    let state: StateHandle = Arc::new((
        Mutex::new(UpdaterState {
            view,
            phase: WindowPhase::Running,
        }),
        Condvar::new(),
    ));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([460.0, 380.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Cromite Portable Updater",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            setup_visuals(&cc.egui_ctx, accent);

            let state_bg = Arc::clone(&state);
            let ctx_bg = cc.egui_ctx.clone();
            std::thread::spawn(move || start_pipeline(state_bg, ctx_bg));

            Ok(Box::new(UpdaterWindow {
                state,
                logo: None,
                taskbar: crate::taskbar::TaskbarProgress::new(),
                hwnd: None,
            }))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_cromite_icon_decodes() {
        let (rgba, w, h) =
            decode_ico_rgba(crate::ICON, 64).expect("app.ico must decode for the window logo");
        assert!(w > 0 && h > 0);
        assert_eq!(rgba.len(), (w * h * 4) as usize);
    }
}
