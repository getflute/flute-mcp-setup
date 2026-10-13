#![windows_subsystem = "windows"]

use eframe::egui;
use flute_mcp_setup::app::SetupApp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Flute MCP Setup")
            .with_inner_size([560.0, 600.0])
            .with_min_inner_size([480.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Flute MCP Setup",
        options,
        Box::new(|cc| Ok(Box::new(SetupApp::new(&cc.egui_ctx)))),
    )
}
