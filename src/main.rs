#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod ui;

fn main() -> anyhow::Result<()> {
    let preview = std::env::args().any(|a| a == "--preview") || bundle_preview();
    let image =
        image::load_from_memory(include_bytes!("../assets/icons/toolbox.png"))?.into_rgba8();
    let (width, height) = image.dimensions();
    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PloegToolbox")
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([760.0, 540.0])
            .with_icon(std::sync::Arc::new(eframe::egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "PloegToolbox",
        native,
        Box::new(move |cc| Ok(Box::new(ui::Toolbox::new(cc, preview)))),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

fn bundle_preview() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let Some(info) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("../Info.plist")))
    else {
        return false;
    };
    std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "PloegDesignPreview", "raw", "-o", "-"])
        .arg(info)
        .output()
        .ok()
        .is_some_and(|result| {
            result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "true"
        })
}
