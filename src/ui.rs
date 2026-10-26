use eframe::egui::{self, Color32, RichText, Vec2};
use ploegtoolbox::{
    catalog::{self, APPS, App, Release},
    install::{self, Installation, Progress},
    store::{Appearance, Settings, Store},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    All,
    Installed,
    Updates,
    Settings,
}

#[derive(Clone, Copy)]
struct Palette {
    canvas: Color32,
    surface: Color32,
    ink: Color32,
    muted: Color32,
    line: Color32,
    accent: Color32,
    soft: Color32,
    on_accent: Color32,
}
impl Palette {
    fn new(dark: bool) -> Self {
        let c = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        if dark {
            Self {
                canvas: c(0x121B2A),
                surface: c(0x1B283C),
                ink: c(0xF1F5FC),
                muted: c(0xACBBD0),
                line: c(0x35455E),
                accent: c(0x89AEFF),
                soft: c(0x293E61),
                on_accent: c(0x10203B),
            }
        } else {
            Self {
                canvas: c(0xEDF2F8),
                surface: c(0xFFFFFF),
                ink: c(0x19283F),
                muted: c(0x58677C),
                line: c(0xD8E0EB),
                accent: c(0x245BD4),
                soft: c(0xE8EFFD),
                on_accent: c(0xFFFFFF),
            }
        }
    }
}

pub struct Toolbox {
    store: Store,
    settings: Settings,
    page: Page,
    selected: usize,
    search: String,
    catalog_input: String,
    releases: Vec<Release>,
    installations: BTreeMap<String, Installation>,
    notice: String,
    error: bool,
    progress: Option<(String, Progress)>,
    receiver: Option<mpsc::Receiver<Progress>>,
    cancel: Arc<AtomicBool>,
    confirm_remove: Option<usize>,
    details_open: bool,
    preview: bool,
}

impl Toolbox {
    pub fn new(cc: &eframe::CreationContext<'_>, preview: bool) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "Ploeg UI".into(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/Inter.ttf")).into(),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "Ploeg UI".into());
        cc.egui_ctx.set_fonts(fonts);
        let store = Store::standard().unwrap_or(Store {
            root: PathBuf::from("PloegToolbox-data"),
            apps: PathBuf::from("PloegToolbox-apps"),
        });
        let (mut settings, mut notice, mut error) = match if preview {
            Ok(Settings::default())
        } else {
            store.settings()
        } {
            Ok(s) => (s, String::new(), false),
            Err(e) => (Settings::default(), e.to_string(), true),
        };
        let args: Vec<_> = std::env::args().collect();
        if args.iter().any(|a| a == "--light") {
            settings.appearance = Appearance::Light;
        }
        if args.iter().any(|a| a == "--dark") {
            settings.appearance = Appearance::Dark;
        }
        if let Some(path) = args.windows(2).find(|v| v[0] == "--catalog") {
            settings.catalog = Some(path[1].clone().into());
        }
        if settings.catalog.is_none() && !preview {
            settings.catalog = bundled_catalog();
        }
        if !preview && let Err(e) = install::recover(&store) {
            notice = format!("Installation recovery needs attention: {e:#}");
            error = true;
        }
        let catalog_input = settings
            .catalog
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut this = Self {
            store,
            settings,
            page: Page::All,
            selected: 0,
            search: String::new(),
            catalog_input,
            releases: vec![],
            installations: BTreeMap::new(),
            notice,
            error,
            progress: None,
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_remove: None,
            details_open: false,
            preview,
        };
        this.refresh();
        if preview {
            for app in APPS.iter().take(2) {
                this.installations.insert(
                    app.id.into(),
                    Installation {
                        path: format!("/Applications/{}.app", app.name).into(),
                        version: "1.0.0".into(),
                        managed: false,
                        toolbox_open_version: 1,
                    },
                );
            }
            this.notice = "Preview · actions disabled".into();
        }
        this
    }

    fn refresh(&mut self) {
        if self.preview {
            return;
        }
        self.installations = APPS
            .iter()
            .filter_map(|a| install::discover(&self.store, a).map(|i| (a.id.into(), i)))
            .collect();
        if let Some(path) = &self.settings.catalog {
            match catalog::load(path) {
                Ok(r) => self.releases = r,
                Err(e) => {
                    self.notice = format!("{e:#}");
                    self.error = true;
                }
            }
        }
    }

    fn save(&mut self) {
        if !self.preview
            && let Err(e) = self.store.save(&self.settings)
        {
            self.notice = format!("Could not save preferences: {e:#}");
            self.error = true;
        }
    }

    fn release(&self, app: &App) -> Option<Release> {
        self.releases
            .iter()
            .filter(|r| r.app == app.id && r.compatible())
            .max_by(|a, b| {
                semver::Version::parse(&catalog::normalized_version(&a.version))
                    .ok()
                    .cmp(&semver::Version::parse(&catalog::normalized_version(&b.version)).ok())
                    .then_with(|| a.toolbox_open_version.cmp(&b.toolbox_open_version))
            })
            .cloned()
    }

    fn has_update(&self, app: &App) -> bool {
        self.installations
            .get(app.id)
            .zip(self.release(app))
            .is_some_and(|(i, r)| install::update_available(i, &r))
    }

    fn open(&mut self, app: &App) {
        if self.preview {
            return;
        }
        let Some(installed) = self.installations.get(app.id).cloned() else {
            return;
        };
        if installed.managed {
            let mut external =
                vec![PathBuf::from("/Applications").join(format!("{}.app", app.name))];
            if let Some(home) = directories::BaseDirs::new() {
                external.push(
                    home.home_dir()
                        .join("Applications")
                        .join(format!("{}.app", app.name)),
                );
            }
            if external.iter().any(|path| install::is_running(path)) {
                self.notice = format!(
                    "Quit the copy of {} that is already running, then try Open again.",
                    app.name
                );
                self.error = false;
                return;
            }
        }
        if let Err(error) = install::open_app(&installed.path, app) {
            self.notice = format!("{error:#}");
            self.error = true;
        } else if installed.toolbox_open_version == 0 {
            self.notice = format!(
                "{} is in the menu bar. Install the updated build to open its window from the Toolbox.",
                app.name
            );
            self.error = false;
            self.details_open = true;
        } else {
            self.notice.clear();
            self.error = false;
        }
    }

    fn start_remove(&mut self, ctx: &egui::Context, app: &App) {
        if self.preview || self.receiver.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress = Some((app.name.into(), Progress::Removing));
        let store = self.store.clone();
        let app = app.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match install::remove(&store, &app) {
                Ok(()) => Progress::Removed,
                Err(error) => Progress::Failed(format!("{error:#}")),
            };
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn start_install(&mut self, ctx: &egui::Context, app: &App) {
        if self.preview || self.receiver.is_some() {
            return;
        }
        let Some(release) = self.release(app) else {
            return;
        };
        if let Err(e) = release.validate() {
            self.notice = format!("Cannot install: {e:#}");
            self.error = true;
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress = Some((app.id.into(), Progress::Downloading(0, 0)));
        self.cancel = Arc::new(AtomicBool::new(false));
        self.notice.clear();
        self.error = false;
        let cancel = self.cancel.clone();
        let store = self.store.clone();
        let app = app.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let outcome = install::install(&store, &app, &release, cancel.clone(), |p| {
                let _ = sender.send(p);
                ctx.request_repaint();
            });
            let result = match outcome {
                Ok(path) => Progress::Complete(path),
                Err(_) if cancel.load(Ordering::Relaxed) => Progress::Cancelled,
                Err(e) => Progress::Failed(format!("{e:#}")),
            };
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let events: Vec<_> = self
            .receiver
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            let done = matches!(
                event,
                Progress::Complete(_)
                    | Progress::Failed(_)
                    | Progress::Cancelled
                    | Progress::Removed
            );
            match &event {
                Progress::Complete(p) => {
                    self.notice = format!("Installed in {}", p.display());
                    self.error = false;
                }
                Progress::Failed(e) => {
                    self.notice = e.clone();
                    self.error = true;
                }
                Progress::Removed => {
                    self.notice = "Managed app removed. Personal data was kept.".into();
                    self.error = false;
                }
                Progress::Cancelled => {
                    self.notice = "Installation cancelled. Your existing app was preserved.".into();
                    self.error = false;
                }
                _ => {}
            }
            if done {
                self.receiver = None;
                self.progress = None;
                self.refresh();
            } else if let Some((_, p)) = &mut self.progress {
                *p = event;
            }
        }
    }

    fn icon(ui: &mut egui::Ui, app: &App, size: f32) {
        ui.add(
            egui::Image::from_bytes(format!("bytes://{}-approved.png", app.id), app.icon)
                .fit_to_exact_size(Vec2::splat(size)),
        );
    }

    fn button(ui: &mut egui::Ui, text: &str, p: Palette, primary: bool) -> egui::Response {
        ui.add(
            egui::Button::new(RichText::new(text).size(13.0).color(if primary {
                p.on_accent
            } else {
                p.ink
            }))
            .fill(if primary { p.accent } else { p.surface })
            .stroke(egui::Stroke::new(
                1.0,
                if primary { p.accent } else { p.line },
            ))
            .corner_radius(8)
            .min_size(egui::vec2(76.0, 36.0)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn sidebar(&mut self, ctx: &egui::Context, p: Palette) {
        egui::SidePanel::left("navigation")
            .exact_width(208.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::from_bytes(
                            "bytes://toolbox.png",
                            include_bytes!("../assets/icons/toolbox.png"),
                        )
                        .fit_to_exact_size(Vec2::splat(32.0)),
                    );
                    ui.label(
                        RichText::new("PloegToolbox")
                            .size(15.0)
                            .strong()
                            .color(p.ink),
                    );
                });
                ui.add_space(32.0);
                for (page, label) in [
                    (Page::All, "All apps"),
                    (Page::Installed, "Installed"),
                    (Page::Updates, "Updates"),
                    (Page::Settings, "Settings"),
                ] {
                    let selected = self.page == page;
                    let response = ui.add_sized(
                        [ui.available_width(), 42.0],
                        egui::Button::new(RichText::new(label).size(14.0).color(if selected {
                            p.accent
                        } else {
                            p.muted
                        }))
                        .fill(if selected {
                            p.soft
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(8),
                    );
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        self.page = page;
                        self.search.clear();
                        self.details_open = false;
                    }
                    ui.add_space(5.0);
                }
            });
    }

    fn icon_button(
        ui: &mut egui::Ui,
        icon: UtilityIcon,
        label: &str,
        p: Palette,
    ) -> egui::Response {
        let response = ui.add_sized(
            [36.0, 36.0],
            egui::Button::new("").frame(false).corner_radius(8),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        paint_icon(ui, response.rect.center(), icon, p.muted);
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
    }

    fn search_ui(&mut self, ui: &mut egui::Ui, width: f32, p: Palette) {
        egui::Frame::new()
            .fill(p.canvas)
            .stroke(egui::Stroke::new(1.0, p.line))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(width - 24.0);
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, 24.0), egui::Sense::hover());
                    paint_icon(ui, rect.center(), UtilityIcon::Search, p.muted);
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .id(egui::Id::new("app-search"))
                            .hint_text(RichText::new("Search apps").color(p.muted))
                            .font(egui::FontId::proportional(14.0))
                            .frame(false)
                            .margin(0)
                            .vertical_align(egui::Align::Center)
                            .desired_width((width - 50.0).max(60.0))
                            .min_size(egui::vec2(0.0, 24.0)),
                    );
                    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                        search.request_focus();
                    }
                });
            });
    }

    fn details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: Palette) {
        let app = APPS[self.selected].clone();
        let installation = self.installations.get(app.id).cloned();
        let release = self.release(&app);
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            Self::icon(ui, &app, 56.0);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(app.name).size(22.0).strong());
                ui.label(RichText::new(app.summary).size(13.0).color(p.muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Self::icon_button(ui, UtilityIcon::Close, "Close details", p).clicked() {
                    self.details_open = false;
                }
            });
        });
        ui.add_space(18.0);
        ui.label(RichText::new(app.description).size(14.0).color(p.muted));
        ui.add_space(18.0);
        if let Some(i) = &installation {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!self.preview, |ui| {
                    if Self::button(ui, "Open", p, true).clicked() {
                        self.open(&app);
                    }
                });
                ui.label(
                    RichText::new(format!("Version {}", i.version))
                        .size(12.0)
                        .color(p.muted),
                );
            });
        }
        if let Some(r) = &release {
            let update = self.has_update(&app);
            if installation.is_none() || update || installation.as_ref().is_some_and(|i| !i.managed)
            {
                if installation.is_some() {
                    ui.add_space(12.0);
                }
                let label = if update {
                    "Update"
                } else if installation.is_some() {
                    "Install another copy"
                } else {
                    "Install"
                };
                let eligibility = r.validate();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(eligibility.is_ok() && self.receiver.is_none() && !self.preview, |ui| {
                        if Self::button(ui, label, p, installation.is_none()).on_hover_text(
                            if installation.is_some() && !update {
                                "Install the new build in the Toolbox app folder. Keep the existing copy."
                            } else {
                                "Install in the Toolbox app folder"
                            }
                        ).clicked() {
                            self.start_install(ctx, &app);
                        }
                    });
                    ui.label(RichText::new(format!("{} · {}", if r.is_remote() {"Release"} else {"Local build"}, r.version)).size(12.0).color(p.muted));
                });
                if let Err(reason) = eligibility {
                    ui.label(RichText::new(reason.to_string()).size(12.0).color(p.muted));
                }
            }
        } else if installation.is_none() {
            ui.label(RichText::new("No build available for this device.").color(p.muted));
        }
        ui.add_space(20.0);
        ui.separator();
        ui.add_space(8.0);
        ui.collapsing("Setup", |ui| {
            ui.label(RichText::new(app.setup).size(13.0).color(p.muted));
        })
        .header_response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.collapsing("App info", |ui| {
            ui.label(
                RichText::new(format!("macOS {}+", app.minimum_macos))
                    .size(12.0)
                    .color(p.muted),
            );
            ui.label(RichText::new(app.privacy).size(13.0).color(p.muted));
            if let Some(i) = &installation {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(i.path.display().to_string())
                        .size(12.0)
                        .color(p.muted),
                );
                if i.managed {
                    ui.add_space(8.0);
                    ui.add_enabled_ui(self.receiver.is_none() && !self.preview, |ui| {
                        if ui.link("Remove app…").clicked() {
                            self.confirm_remove = Some(self.selected);
                            self.details_open = false;
                        }
                    });
                }
            }
        })
        .header_response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
        if let Some(r) = release {
            if !r.notes.trim().is_empty() {
                ui.collapsing("Release notes", |ui| {
                    ui.label(RichText::new(&r.notes).size(13.0).color(p.muted));
                })
                .header_response
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            if installation.is_some_and(|i| i.managed) && !self.has_update(&app) {
                ui.collapsing("Installation", |ui| {
                    ui.label(
                        RichText::new(format!("Available build · {}", r.version))
                            .size(12.0)
                            .color(p.muted),
                    );
                    ui.add_enabled_ui(
                        r.validate().is_ok() && self.receiver.is_none() && !self.preview,
                        |ui| {
                            if Self::button(ui, "Reinstall", p, false).clicked() {
                                self.start_install(ctx, &app);
                            }
                        },
                    );
                })
                .header_response
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            }
        }
    }

    fn catalog_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: Palette) {
        let title = match self.page {
            Page::All => "Apps",
            Page::Installed => "Installed",
            Page::Updates => "Updates",
            Page::Settings => "Settings",
        };
        let wide = ui.available_width() >= 560.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(26.0).strong().color(p.ink));
            if wide {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Self::icon_button(ui, UtilityIcon::Refresh, "Refresh apps", p).clicked() {
                        self.refresh();
                    }
                    self.search_ui(ui, 250.0, p);
                });
            }
        });
        if !wide {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                self.search_ui(ui, (ui.available_width() - 44.0).max(140.0), p);
                if Self::icon_button(ui, UtilityIcon::Refresh, "Refresh apps", p).clicked() {
                    self.refresh();
                }
            });
        }
        ui.add_space(28.0);
        let mut shown = 0;
        for (index, app) in APPS.iter().enumerate() {
            let installed = self.installations.contains_key(app.id);
            if self.page == Page::Installed && !installed
                || self.page == Page::Updates && !self.has_update(app)
                || !format!("{} {}", app.name, app.summary)
                    .to_lowercase()
                    .contains(&self.search.to_lowercase())
            {
                continue;
            }
            if shown > 0 {
                ui.separator();
            }
            shown += 1;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(0, 18))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        Self::icon(ui, app, 56.0);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(app.name).strong().size(18.0).color(p.ink),
                                    )
                                    .frame(false),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.selected = index;
                                self.details_open = true;
                            }
                            ui.label(RichText::new(app.summary).size(13.0).color(p.muted));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if Self::icon_button(
                                ui,
                                UtilityIcon::More,
                                &format!("{} details", app.name),
                                p,
                            )
                            .clicked()
                            {
                                self.selected = index;
                                self.details_open = true;
                            }
                            let label = if self.has_update(app) {
                                "Update"
                            } else if installed {
                                "Open"
                            } else if self.release(app).is_some() {
                                "Install"
                            } else {
                                "Details"
                            };
                            ui.add_enabled_ui(!self.preview || label == "Details", |ui| {
                                if Self::button(ui, label, p, true).clicked() {
                                    self.selected = index;
                                    if self.has_update(app)
                                        || !installed && self.release(app).is_some()
                                    {
                                        self.start_install(ctx, app);
                                    } else if installed {
                                        self.open(app);
                                    } else {
                                        self.details_open = true;
                                    }
                                }
                            });
                        });
                    });
                });
        }
        if shown == 0 {
            ui.add_space(32.0);
            let message = if !self.search.is_empty() {
                "No apps found."
            } else if self.page == Page::Updates {
                "No updates in this catalog."
            } else {
                "No apps installed."
            };
            ui.label(RichText::new(message).color(p.muted));
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.label(RichText::new("Settings").size(26.0).strong());
        ui.add_space(28.0);
        ui.heading("Appearance");
        let before = self.settings.appearance;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.settings.appearance, Appearance::System, "System")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.selectable_value(&mut self.settings.appearance, Appearance::Light, "Light")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.selectable_value(&mut self.settings.appearance, Appearance::Dark, "Dark")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        });
        if before != self.settings.appearance {
            self.save();
        }
        ui.add_space(28.0);
        ui.separator();
        ui.add_space(20.0);
        ui.collapsing("Advanced · app sources", |ui| {
        ui.heading("Release catalog");
        ui.label(RichText::new("Use a Ploeg catalog from a source you trust. Packages are verified before installation.").size(13.0).color(p.muted));
        ui.add_space(12.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.catalog_input)
                .hint_text("Path to catalog.json")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(10.0);
        if Self::button(ui, "Load catalog", p, true).clicked() {
            let path = PathBuf::from(self.catalog_input.trim());
            match catalog::load(&path) {
                Ok(releases) => {
                    self.releases = releases;
                    self.settings.catalog = Some(path);
                    self.save();
                    self.notice = "Release catalog loaded.".into();
                    self.error = false;
                }
                Err(e) => {
                    self.notice = format!("{e:#}");
                    self.error = true;
                }
            }
        }
        }).header_response.on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.add_space(28.0);
        ui.separator();
        ui.add_space(20.0);
        ui.heading("App folder");
        ui.label(
            RichText::new(self.store.apps.display().to_string())
                .size(12.0)
                .color(p.muted),
        );
        ui.label(
            RichText::new("Removing an app from this folder keeps its data.")
                .size(13.0)
                .color(p.muted),
        );
        ui.add_space(10.0);
        if Self::button(ui, "Show app folder", p, false).clicked() && !self.preview {
            let result = std::fs::create_dir_all(&self.store.apps)
                .map_err(anyhow::Error::from)
                .and_then(|_| install::open(&self.store.apps));
            if let Err(e) = result {
                self.notice = e.to_string();
                self.error = true;
            }
        }
        ui.add_space(26.0);
        ui.label(
            RichText::new(format!("PloegToolbox 0.1.0 · {}", platform_label()))
                .size(12.0)
                .color(p.muted),
        );
    }
}

impl eframe::App for Toolbox {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        let theme = match self.settings.appearance {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Light => egui::ThemePreference::Light,
            Appearance::Dark => egui::ThemePreference::Dark,
        };
        ctx.set_theme(theme);
        let p = Palette::new(ctx.style().visuals.dark_mode);
        ctx.style_mut(|s| {
            s.visuals.panel_fill = p.surface;
            s.visuals.window_fill = p.surface;
            s.visuals.extreme_bg_color = p.canvas;
            s.visuals.faint_bg_color = p.canvas;
            s.visuals.override_text_color = Some(p.ink);
            s.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.line);
            s.visuals.widgets.inactive.bg_fill = p.surface;
            s.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, p.line);
            s.visuals.widgets.hovered.bg_fill = p.soft;
            s.visuals.selection.bg_fill = p.soft;
            s.visuals.hyperlink_color = p.accent;
            s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        });
        if self.progress.is_some() || !self.notice.is_empty() {
            egui::TopBottomPanel::bottom("status")
                .frame(
                    egui::Frame::new()
                        .fill(p.canvas)
                        .inner_margin(egui::Margin::symmetric(18, 10)),
                )
                .show(ctx, |ui| {
                    if let Some((id, progress)) = &self.progress {
                        let title = match progress {
                            Progress::Downloading(done, total) => {
                                if *total > 0 {
                                    format!(
                                        "Downloading · {:.0}%",
                                        *done as f64 / *total as f64 * 100.0
                                    )
                                } else {
                                    "Preparing download…".into()
                                }
                            }
                            Progress::Verifying => "Verifying package…".into(),
                            Progress::Installing => "Installing verified app…".into(),
                            Progress::Removing => "Removing managed app…".into(),
                            _ => String::new(),
                        };
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(format!("{id} · {title}"));
                            if !matches!(progress, Progress::Installing | Progress::Removing)
                                && ui
                                    .button("Cancel")
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                            {
                                self.cancel.store(true, Ordering::Relaxed);
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&self.notice).size(12.0).color(if self.error {
                                Color32::from_rgb(190, 68, 82)
                            } else {
                                p.muted
                            }));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if Self::icon_button(
                                        ui,
                                        UtilityIcon::Close,
                                        "Dismiss message",
                                        p,
                                    )
                                    .clicked()
                                    {
                                        self.notice.clear();
                                    }
                                },
                            );
                        });
                    }
                });
        }
        self.sidebar(ctx, p);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin::same(28)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("main scroll")
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(760.0));
                        ui.add_space(12.0);
                        if self.page == Page::Settings {
                            self.settings_ui(ui, p);
                        } else {
                            self.catalog_ui(ui, ctx, p);
                        }
                    });
            });
        if let Some(index) = self.confirm_remove {
            egui::Modal::new(egui::Id::new("confirm removal")).show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.heading(format!("Remove {}?", APPS[index].name));
                ui.label("This removes only the managed application. Preferences, models, history and documents are kept. Your external copy is left in place.");
                ui.horizontal(|ui| {
                    if ui.button("Keep app").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.confirm_remove = None;
                    }
                    if ui.button("Remove managed app").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.confirm_remove = None;
                        self.start_remove(ctx, &APPS[index]);
                    }
                });
            });
        }
        if self.details_open {
            let response = egui::Modal::new(egui::Id::new("app-details"))
                .frame(
                    egui::Frame::popup(&ctx.style())
                        .inner_margin(24)
                        .corner_radius(14),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("details-scroll")
                        .max_height((ctx.content_rect().height() - 100.0).max(280.0))
                        .show(ui, |ui| self.details(ui, ctx, p));
                });
            if response.should_close() {
                self.details_open = false;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum UtilityIcon {
    Search,
    Refresh,
    More,
    Close,
}

fn paint_icon(ui: &egui::Ui, center: egui::Pos2, icon: UtilityIcon, color: Color32) {
    let painter = ui.painter();
    let stroke = egui::Stroke::new(1.5, color);
    match icon {
        UtilityIcon::Search => {
            painter.circle_stroke(center + egui::vec2(-2.0, -2.0), 4.5, stroke);
            painter.line_segment(
                [center + egui::vec2(1.5, 1.5), center + egui::vec2(5.0, 5.0)],
                stroke,
            );
        }
        UtilityIcon::Refresh => {
            let points = (0..=24)
                .map(|i| {
                    let angle = 0.5 + i as f32 / 24.0 * 5.1;
                    center + egui::vec2(angle.cos(), angle.sin()) * 6.0
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            painter.line_segment(
                [
                    center + egui::vec2(4.7, -7.0),
                    center + egui::vec2(4.7, -3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(4.7, -3.0),
                    center + egui::vec2(0.7, -3.0),
                ],
                stroke,
            );
        }
        UtilityIcon::More => {
            for x in [-5.0, 0.0, 5.0] {
                painter.circle_filled(center + egui::vec2(x, 0.0), 1.5, color);
            }
        }
        UtilityIcon::Close => {
            painter.line_segment(
                [
                    center + egui::vec2(-4.0, -4.0),
                    center + egui::vec2(4.0, 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(4.0, -4.0),
                    center + egui::vec2(-4.0, 4.0),
                ],
                stroke,
            );
        }
    }
}

fn platform_label() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "macOS · Apple silicon"
        } else {
            "macOS · Intel"
        }
    } else if cfg!(target_os = "windows") {
        "Windows · utility ports planned"
    } else {
        "Linux · utility ports planned"
    }
}

fn bundled_catalog() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let path = executable.parent()?.join("../Resources/catalog.json");
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button_cursor(icon: bool, enabled: bool) -> egui::CursorIcon {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    rect = if icon {
                        Toolbox::icon_button(
                            ui,
                            UtilityIcon::More,
                            "App details",
                            Palette::new(false),
                        )
                        .rect
                    } else {
                        Toolbox::button(ui, "Open", Palette::new(false), true).rect
                    };
                });
            });
        });
        // Use the actual laid-out hit target, then run a real hover frame.
        let point = rect.center();
        ctx.run(
            egui::RawInput {
                events: vec![egui::Event::PointerMoved(point)],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        if icon {
                            Toolbox::icon_button(
                                ui,
                                UtilityIcon::More,
                                "App details",
                                Palette::new(false),
                            );
                        } else {
                            Toolbox::button(ui, "Open", Palette::new(false), true);
                        }
                    });
                });
            },
        )
        .platform_output
        .cursor_icon
    }

    #[test]
    fn primary_action_uses_hand_pointer() {
        assert_eq!(button_cursor(false, true), egui::CursorIcon::PointingHand);
    }

    #[test]
    fn icon_action_uses_hand_pointer() {
        assert_eq!(button_cursor(true, true), egui::CursorIcon::PointingHand);
    }

    #[test]
    fn disabled_actions_do_not_use_hand_pointer() {
        assert_eq!(button_cursor(false, false), egui::CursorIcon::Default);
        assert_eq!(button_cursor(true, false), egui::CursorIcon::Default);
    }
}
