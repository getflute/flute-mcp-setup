//! The window. Holds UI state only; all work happens on worker threads that
//! report back through `Event`s.

use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke, TextStyle};

use crate::catalog::McpKind;
use crate::configure::{self, Event, Outcome};
use crate::detect::agents::{AgentKind, AgentStatus, join_labels};
use crate::env::Env;
use crate::marks::{MISSING, OK, PENDING};
use crate::process::SystemRunner;

const GREEN: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x43);
const AMBER: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
const RED: Color32 = Color32::from_rgb(0xd0, 0x3b, 0x3b);
/// Space between the window edge and the content, in points.
const MARGIN: i8 = 24;
const BUTTON_SIZE: egui::Vec2 = egui::vec2(150.0, 38.0);

pub struct SetupApp {
    webhooks: bool,
    flute_cli: bool,
    agents: Option<Vec<AgentStatus>>,
    log: String,
    outcome: Option<Outcome>,
    running: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl SetupApp {
    /// Opens the window state and starts agent detection in the background.
    pub fn new(ctx: &egui::Context) -> SetupApp {
        apply_style(ctx);
        let app = SetupApp::idle();
        let tx = app.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let event = match Env::from_system() {
                Ok(env) => Event::Agents(configure::detect_agents(&env, &SystemRunner)),
                Err(e) => Event::Log(format!("{MISSING} {e}")),
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
        app
    }

    fn idle() -> SetupApp {
        let (tx, rx) = channel();
        SetupApp {
            webhooks: false,
            flute_cli: false,
            agents: None,
            log: String::new(),
            outcome: None,
            running: false,
            tx,
            rx,
        }
    }

    pub fn can_configure(&self) -> bool {
        (self.webhooks || self.flute_cli) && !self.running
    }

    fn selected(&self) -> Vec<McpKind> {
        let mut selected = Vec::new();
        if self.webhooks {
            selected.push(McpKind::Webhooks);
        }
        if self.flute_cli {
            selected.push(McpKind::FluteCli);
        }
        selected
    }

    fn start_configure(&mut self, ctx: egui::Context) {
        self.running = true;
        self.outcome = None;
        self.log.clear();
        let selected = self.selected();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut emit = |event: Event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            let outcome = match Env::from_system() {
                Ok(env) => {
                    let now = chrono::Local::now().naive_local();
                    configure::run(&selected, &env, &SystemRunner, now, &mut emit)
                }
                Err(e) => {
                    emit(Event::Log(format!("{MISSING} {e}")));
                    Outcome::Failed
                }
            };
            emit(Event::Done(outcome));
        });
    }

    /// Apply every pending event from the workers.
    fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.apply(event);
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Agents(agents) => self.agents = Some(agents),
            Event::Log(line) => {
                self.log.push_str(&line);
                self.log.push('\n');
            }
            Event::Done(outcome) => {
                self.running = false;
                self.outcome = Some(outcome);
            }
        }
    }

    fn status_row(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("AI apps:").strong());
            match &self.agents {
                None => {
                    for kind in AgentKind::ALL {
                        ui.label(RichText::new(format!("{PENDING} {}", kind.label())).weak())
                            .on_hover_text("checking…");
                    }
                }
                Some(agents) => {
                    for agent in agents {
                        let (mark, color) = if agent.found() {
                            (OK, GREEN)
                        } else {
                            (MISSING, Color32::GRAY)
                        };
                        ui.label(
                            RichText::new(format!("{mark} {}", agent.kind.label())).color(color),
                        )
                        .on_hover_text(&agent.detail);
                    }
                }
            }
        });
    }
}

/// Larger text and roomier spacing than egui's defaults, in both themes.
pub fn apply_style(ctx: &egui::Context) {
    use egui::FontFamily::{Monospace, Proportional};
    use egui::FontId;
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(26.0, Proportional)),
            (TextStyle::Body, FontId::new(17.0, Proportional)),
            (TextStyle::Button, FontId::new(17.0, Proportional)),
            (TextStyle::Monospace, FontId::new(14.0, Monospace)),
            (TextStyle::Small, FontId::new(13.0, Proportional)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.icon_width = 20.0;
        style.spacing.icon_width_inner = 12.0;
        style.spacing.icon_spacing = 8.0;
    });
}

/// A fixed-size button whose label is centred both ways. A button's label
/// follows the alignment of the layout it sits in, so give it a centred one.
fn centered_button(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &str,
    fill: Option<Color32>,
) -> egui::Response {
    let layout = Layout::centered_and_justified(egui::Direction::TopDown);
    ui.allocate_ui_with_layout(BUTTON_SIZE, layout, |ui| {
        let mut button = egui::Button::new(text).min_size(BUTTON_SIZE);
        if let Some(fill) = fill {
            button = button.fill(fill);
        }
        ui.add_enabled(enabled, button)
    })
    .inner
}

/// Banner colour and text for a finished run.
pub fn banner(outcome: &Outcome) -> (Color32, String) {
    match outcome {
        Outcome::Success {
            agents,
            servers,
            changed: true,
        } => {
            let names = join_labels(agents);
            (
                GREEN,
                format!("Configured {servers} servers for {names}. Restart {names} to load them."),
            )
        }
        Outcome::Success {
            agents,
            changed: false,
            ..
        } => (
            GREEN,
            format!(
                "Already up to date — no changes needed for {}.",
                join_labels(agents)
            ),
        ),
        Outcome::Partial => (
            AMBER,
            "Some apps could not be configured — see the log.".to_string(),
        ),
        Outcome::Failed => (RED, "Configuration failed — see the log.".to_string()),
        Outcome::NothingChanged => (
            RED,
            "Missing requirements — nothing was changed. See the log for install instructions."
                .to_string(),
        ),
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let succeeded = matches!(self.outcome, Some(Outcome::Success { .. }));
        if self.running && ui.input(|i| i.viewport().close_requested()) {
            // Closing between `claude mcp remove` and `add-json` would lose the entry.
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        let footer_frame = egui::Frame::side_top_panel(ui.style())
            .inner_margin(egui::Margin::symmetric(MARGIN, 12));
        egui::Panel::bottom("footer")
            .frame(footer_frame)
            .show(ui, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let fill = succeeded.then(|| ui.visuals().selection.bg_fill);
                    if centered_button(ui, !self.running, "Exit", fill).clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });

        let central_frame = egui::Frame::central_panel(ui.style()).inner_margin(MARGIN);
        egui::CentralPanel::default()
            .frame(central_frame)
            .show(ui, |ui| {
                ui.heading("Flute MCP Setup");
                ui.add_space(8.0);
                self.status_row(ui);
                ui.add_space(12.0);

                ui.add_enabled_ui(!self.running, |ui| {
                    ui.checkbox(&mut self.webhooks, McpKind::Webhooks.label());
                    ui.checkbox(&mut self.flute_cli, McpKind::FluteCli.label());
                });
                ui.add_space(12.0);

                let label = if self.running {
                    "Configuring…"
                } else {
                    "Configure"
                };
                if centered_button(ui, self.can_configure(), label, None).clicked() {
                    self.start_configure(ui.ctx().clone());
                }

                if let Some(outcome) = &self.outcome {
                    ui.add_space(10.0);
                    let (color, text) = banner(outcome);
                    egui::Frame::group(ui.style())
                        .stroke(Stroke::new(1.5, color))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(text).color(color).strong());
                        });
                }

                ui.add_space(10.0);
                ui.label("Output");
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .corner_radius(4)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.log.as_str())
                                        .frame(egui::Frame::NONE)
                                        .font(TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(1)
                                        .min_size(ui.available_size()),
                                );
                            });
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// Run `frames` frames of the app at 560×600 and return the last frame's shapes.
    fn render(app: &mut SetupApp, frames: usize) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(560.0, 600.0));
        let mut shapes = Vec::new();
        for _ in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
            shapes = out.shapes;
        }
        shapes
    }

    fn flatten(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    flatten(s, out);
                }
            }
            other => out.push(other.clone()),
        }
    }

    fn all_shapes(clipped: &[egui::epaint::ClippedShape]) -> Vec<egui::Shape> {
        let mut out = Vec::new();
        for c in clipped {
            flatten(&c.shape, &mut out);
        }
        out
    }

    /// The laid-out rect of the first text shape whose text starts with `needle`.
    fn text_rect(shapes: &[egui::Shape], needle: &str) -> egui::Rect {
        shapes
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text().starts_with(needle) => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text starting with {needle:?}"))
    }

    /// The smallest filled rect that contains `inner` (a button or field background).
    fn enclosing_rect(shapes: &[egui::Shape], inner: egui::Rect) -> egui::epaint::RectShape {
        shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.rect.contains_rect(inner) && r.fill.a() > 0 => {
                    Some(r.clone())
                }
                _ => None,
            })
            .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
            .expect("no filled rect encloses the text")
    }

    #[test]
    fn style_uses_larger_text() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let style = ctx.global_style();
        let size = |s: TextStyle| style.text_styles[&s].size;
        assert!(size(TextStyle::Heading) >= 24.0);
        assert!(size(TextStyle::Body) >= 16.0);
        assert!(size(TextStyle::Button) >= 16.0);
        assert!(size(TextStyle::Monospace) >= 13.5);
    }

    #[test]
    fn button_labels_are_centred() {
        let shapes = all_shapes(&render(&mut SetupApp::idle(), 3));
        for label in ["Configure", "Exit"] {
            let text = text_rect(&shapes, label);
            let button = enclosing_rect(&shapes, text).rect;
            let offset = text.center() - button.center();
            assert!(
                offset.x.abs() <= 1.5 && offset.y.abs() <= 1.5,
                "{label} label is off-centre by {offset:?} (text {text:?}, button {button:?})"
            );
        }
    }

    #[test]
    fn content_has_a_comfortable_left_margin() {
        let shapes = all_shapes(&render(&mut SetupApp::idle(), 3));
        let heading = text_rect(&shapes, "Flute MCP Setup");
        assert!(
            heading.min.x >= 20.0,
            "heading starts at x={}",
            heading.min.x
        );
    }

    #[test]
    fn log_sits_in_a_visible_field() {
        let mut app = SetupApp::idle();
        app.log = "hello from the log\n".to_string();
        let shapes = all_shapes(&render(&mut app, 3));
        let text = text_rect(&shapes, "hello from the log");
        let field = enclosing_rect(&shapes, text);
        assert!(
            field.stroke.width > 0.0 && field.stroke.color.a() > 0,
            "field has no border: {field:?}"
        );
        assert!(
            field.rect.height() >= 200.0,
            "field is too short: {:?}",
            field.rect
        );
    }

    #[test]
    fn long_log_shows_its_newest_line() {
        let mut app = SetupApp::idle();
        app.log = (0..200).fold(String::new(), |mut log, i| {
            let _ = writeln!(log, "line {i}");
            log
        });
        let shapes = all_shapes(&render(&mut app, 4));
        let field = shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.fill.a() > 0 && r.stroke.width > 0.0 => Some(r.rect),
                _ => None,
            })
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .expect("no outlined field");
        let galley_bottom = shapes
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text().starts_with("line 0") => {
                    Some(t.pos.y + t.galley.size().y)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            (galley_bottom - field.max.y).abs() <= 20.0,
            "newest line is not at the bottom of the field: text ends at {galley_bottom}, field ends at {}",
            field.max.y
        );
    }

    /// Run one frame with a window-close request; return the root viewport's commands.
    fn commands_after_close_request(app: &mut SetupApp) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(560.0, 600.0),
            )),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        out.textures_delta.clear();
        out.viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    /// Closing between `claude mcp remove` and `add-json` would lose the entry.
    #[test]
    fn closing_is_refused_while_configuring() {
        let mut app = SetupApp::idle();
        app.running = true;
        let commands = commands_after_close_request(&mut app);
        assert!(
            commands.contains(&egui::ViewportCommand::CancelClose),
            "{commands:?}"
        );
    }

    #[test]
    fn closing_is_allowed_when_idle() {
        let mut app = SetupApp::idle();
        let commands = commands_after_close_request(&mut app);
        assert!(
            !commands.contains(&egui::ViewportCommand::CancelClose),
            "{commands:?}"
        );
    }

    /// Panels persist their size between frames, so content that claims the
    /// panel's full height makes it grow on every repaint.
    #[expect(
        clippy::float_cmp,
        reason = "layout must be bit-for-bit identical across frames"
    )]
    #[test]
    fn footer_keeps_a_stable_small_height_across_frames() {
        let ctx = egui::Context::default();
        let mut app = SetupApp::idle();
        let mut frame = eframe::Frame::_new_kittest();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(560.0, 600.0));
        let mut heights = Vec::new();
        for _ in 0..10 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            out.textures_delta.clear();
            let state = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("footer"));
            heights.push(state.expect("footer panel state").outer_rect.height());
        }
        assert!(
            heights.windows(2).all(|w| w[0] == w[1]),
            "footer height changed: {heights:?}"
        );
        assert!(heights[0] < 80.0, "footer is too tall: {heights:?}");
    }

    #[test]
    fn configure_needs_a_toggle_and_no_run_in_progress() {
        let mut app = SetupApp::idle();
        assert!(!app.can_configure());
        app.webhooks = true;
        assert!(app.can_configure());
        app.webhooks = false;
        app.flute_cli = true;
        assert!(app.can_configure());
        app.running = true;
        assert!(!app.can_configure());
    }

    #[test]
    fn selection_follows_toggles_in_display_order() {
        let mut app = SetupApp::idle();
        app.flute_cli = true;
        app.webhooks = true;
        assert_eq!(app.selected(), [McpKind::Webhooks, McpKind::FluteCli]);
    }

    #[test]
    fn events_update_state() {
        let mut app = SetupApp::idle();
        app.running = true;
        app.tx.send(Event::Log("one".into())).unwrap();
        app.tx.send(Event::Log("two".into())).unwrap();
        app.tx.send(Event::Agents(Vec::new())).unwrap();
        app.tx.send(Event::Done(Outcome::Failed)).unwrap();
        app.poll();
        assert_eq!(app.log, "one\ntwo\n");
        assert_eq!(app.agents, Some(Vec::new()));
        assert!(!app.running);
        assert_eq!(app.outcome, Some(Outcome::Failed));
    }

    #[test]
    fn banner_texts() {
        let all = vec![
            AgentKind::ClaudeDesktop,
            AgentKind::ClaudeCode,
            AgentKind::ChatGpt,
        ];
        let (color, text) = banner(&Outcome::Success {
            agents: all.clone(),
            servers: 4,
            changed: true,
        });
        assert_eq!(color, GREEN);
        assert_eq!(
            text,
            "Configured 4 servers for Claude Desktop, Claude Code and ChatGPT. Restart Claude Desktop, Claude Code and ChatGPT to load them."
        );
        let (_, text) = banner(&Outcome::Success {
            agents: vec![AgentKind::ChatGpt],
            servers: 2,
            changed: false,
        });
        assert_eq!(text, "Already up to date — no changes needed for ChatGPT.");
        assert_eq!(banner(&Outcome::Partial).0, AMBER);
        assert_eq!(banner(&Outcome::Failed).0, RED);
        assert!(
            banner(&Outcome::NothingChanged)
                .1
                .contains("nothing was changed")
        );
    }
}
