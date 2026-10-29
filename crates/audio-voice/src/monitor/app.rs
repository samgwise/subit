//! The monitor window: an egui app rendering the conductor's snapshots.
//! Runs on the **main** thread — winit's event loop refuses to start
//! anywhere else (it panics on Windows, and macOS requires the main
//! thread) — while the conductor runs beside it on its own thread. Each
//! frame borrows the latest snapshot from the watch channel.

use std::time::Duration;

use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, Points, VLine};

use crate::composer::SlotRecord;
use crate::harmony::VOCABULARY;
use crate::monitor::MonitorView;
use crate::snapshot::{HarmonyKind, PlannedSlotView, UiSnapshot};

/// How often the window repaints while idle — smooth enough for the
/// slot-boundary cadence, light enough to ignore.
const REPAINT: Duration = Duration::from_millis(33);

/// Run the window to completion on the **main** thread — winit's event
/// loop must be built there. Returns when the window closes.
pub fn run(view: MonitorView) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("subit — audio voice monitor")
            .with_inner_size([1100.0, 720.0]),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "audio-voice-monitor",
        options,
        Box::new(|_cc| Ok(Box::new(MonitorApp { view }))),
    ) {
        tracing::warn!("the monitor window closed with an error: {err:?}");
    }
}

struct MonitorApp {
    view: MonitorView,
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(REPAINT);

        let Some(snapshot) = self.view.latest() else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("waiting for the conductor…");
            });
            return;
        };

        egui::TopBottomPanel::top("status").show(ctx, |ui| self.status_bar(ui, &snapshot));
        egui::SidePanel::right("composer").show(ctx, |ui| {
            self.composer_panel(ui, &snapshot);
            ui.separator();
            self.event_log(ui, &snapshot);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            self.curve_plots(ui, &snapshot);
            ui.separator();
            self.harmony_strip(ui, &snapshot);
            ui.separator();
            self.history_timeline(ui, &snapshot);
        });
    }
}

impl MonitorApp {
    /// The status bar: key, form, badges and the drive's bars.
    fn status_bar(&self, ui: &mut egui::Ui, snapshot: &UiSnapshot) {
        ui.horizontal_wrapped(|ui| {
            let harmony = &snapshot.harmony;
            ui.label(egui::RichText::new(&harmony.key).strong().heading());
            ui.separator();
            ui.label(format!(
                "{} · {} phrases left",
                harmony.form.state, harmony.form.phrases_left
            ));

            if snapshot.drive.degraded {
                ui.colored_label(egui::Color32::from_rgb(255, 96, 96), "◉ DEGRADED");
            } else {
                ui.weak("◦ clean data");
            }
            if snapshot.drive.episode {
                ui.weak("(episode shade)");
            }

            ui.separator();
            ui.label(format!("aggro {} locks", snapshot.drive.aggro_locks));

            let bar = |ui: &mut egui::Ui, label: &str, value: f32| {
                ui.add_sized(
                    [150.0, 18.0],
                    egui::ProgressBar::new(value.clamp(0.0, 1.0))
                        .text(format!("{label} {:.2}", value)),
                );
            };
            let (intensity, rhythm) = (snapshot.drive.intensity, snapshot.drive.rhythm_intensity);
            bar(ui, "intensity", intensity);
            bar(ui, "rhythm", rhythm);
            if snapshot.drive.rhythm_hold > 0.0 {
                ui.weak(format!("holding {:.1}s", snapshot.drive.rhythm_hold));
            }
            bar(ui, "objective", snapshot.drive.objective);
            ui.weak(format!("span {:.2}", snapshot.curves.span));

            ui.separator();
            let clock = &snapshot.clock;
            ui.weak(format!(
                "slot {} | {:.2}s → {:.2}s ({}s)",
                clock.slot,
                clock.slot_start,
                clock.slot_end,
                (clock.slot_end - clock.hub_time).max(0.0),
            ));
        });
    }

    /// The voicing contour and the dynamic arc, side by side vertically.
    fn curve_plots(&self, ui: &mut egui::Ui, snapshot: &UiSnapshot) {
        let curves = &snapshot.curves;
        let t = curves.phrase_t;

        ui.heading("Phrase curves");
        Plot::new("voicing contour")
            .height(220.0)
            .x_axis_label("phrase parameter")
            .y_axis_label("semitones from tonic")
            .show(ui, |plot_ui| {
                plot_ui.line(
                    Line::new(PlotPoints::from(samples_of(&curves.contour, |s| (s.0, s.2))))
                        .name("upper bound")
                        .color(egui::Color32::from_rgb(96, 220, 255)),
                );
                plot_ui.line(
                    Line::new(PlotPoints::from(samples_of(&curves.contour, |s| (s.0, s.1))))
                        .name("lower bound")
                        .color(egui::Color32::from_rgb(255, 128, 224)),
                );
                plot_ui.points(
                    Points::new(PlotPoints::from(control_points(&curves.upper_control)))
                        .name("upper control")
                        .color(egui::Color32::from_rgb(96, 220, 255))
                        .radius(3.0_f32),
                );
                plot_ui.points(
                    Points::new(PlotPoints::from(control_points(&curves.lower_control)))
                        .name("lower control")
                        .color(egui::Color32::from_rgb(255, 128, 224))
                        .radius(3.0_f32),
                );
                plot_ui.vline(VLine::new(t).name("sounding slot").color(egui::Color32::GRAY));
            })
            .response
            .on_hover_text(format!(
                "the patterns bend into these bounds; the marker is the slot sounding now (t = {t:.3})"
            ));

        Plot::new("dynamic arc")
            .height(110.0)
            .y_axis_label("shape 0–1")
            .include_y(0.0)
            .include_y(1.0)
            .show(ui, |plot_ui| {
                plot_ui.line(
                    Line::new(
                        curves
                            .dynamic_shape
                            .iter()
                            .map(|(t, shape)| [*t, *shape])
                            .collect::<Vec<_>>(),
                    )
                    .name("dynamic shape")
                    .color(egui::Color32::from_rgb(255, 208, 96)),
                );
                plot_ui.vline(VLine::new(t).name("sounding slot").color(egui::Color32::GRAY));
            })
            .response
            .on_hover_text("the phrase's crescendo arc, normalised to its own hull");
    }

    /// The harmony strip: where the chords come from and what's coming.
    fn harmony_strip(&self, ui: &mut egui::Ui, snapshot: &UiSnapshot) {
        ui.heading("Harmony");
        let harmony = &snapshot.harmony;
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("sounding:").weak());
            ui.label(
                egui::RichText::new(format!(
                    "{} [{}]",
                    short_name(harmony.current.name),
                    harmony.current.function
                ))
                .strong(),
            );
            match harmony.source {
                HarmonyKind::Cycle => {
                    ui.colored_label(egui::Color32::from_rgb(150, 200, 150), "cycle");
                    if let Some((position, length)) = harmony.turn_position {
                        ui.weak(format!("turn {position} / {length}"));
                    }
                }
                HarmonyKind::Plan => {
                    ui.colored_label(egui::Color32::from_rgb(150, 170, 255), "plan");
                    ui.weak(format!(
                        "{} slots left (cursor {})",
                        harmony.slots_remaining.unwrap_or_default(),
                        harmony.cursor.unwrap_or_default(),
                    ));
                }
            }
        });
        if let Some(intent) = &harmony.intent
            && harmony.source == HarmonyKind::Plan
        {
            ui.label(egui::RichText::new(format!("intent: {intent}")).italics());
        }

        if harmony.planned.is_empty() {
            ui.weak("the deterministic cycle is playing — no planned slots ahead");
        } else {
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, slot) in harmony.planned.iter().enumerate() {
                        let cursor = harmony.cursor.unwrap_or(0);
                        let state = if index < cursor {
                            CellState::Past
                        } else if index == cursor {
                            CellState::Current
                        } else {
                            CellState::Future
                        };
                        chord_cell(ui, slot, state);
                    }
                });
            });
        }
    }

    /// The recent chord slots — cuts ring red, holds ring blue.
    fn history_timeline(&self, ui: &mut egui::Ui, snapshot: &UiSnapshot) {
        ui.heading("Recent slots");
        if snapshot.history.is_empty() {
            ui.weak("nothing played yet");
            return;
        }
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for record in &snapshot.history {
                    history_cell(ui, record);
                }
            });
        });
    }

    /// The composer panel: enabled, last replan reason, last tallies.
    fn composer_panel(&self, ui: &mut egui::Ui, snapshot: &UiSnapshot) {
        ui.heading("Composer");
        let composer = &snapshot.composer;
        if composer.enabled {
            ui.label("ollama composer listening");
        } else {
            ui.weak("disabled (OLLAMA_DISABLED or unset model)");
        }
        match composer.last_replan {
            Some(reason) => ui.label(format!("last replan: {reason:?}")),
            None => ui.weak("no replan yet"),
        };
        if let Some(tallies) = &composer.last_tallies {
            egui::Grid::new("tallies").num_columns(2).show(ui, |ui| {
                tally_row(ui, "mob sweeps", tallies.mob_sweeps);
                tally_row(ui, "kills", tallies.kills);
                tally_row(ui, "level ups", tallies.level_ups);
                tally_row(ui, "corruption in/out", format_args!(
                    "{} / {}",
                    tallies.corruption_enters, tallies.corruption_exits
                ));
                tally_row(ui, "descents", tallies.descents);
                tally_row(ui, "deaths", tallies.deaths);
                tally_row(ui, "aggro avg/peak", format_args!(
                    "{:.1} / {}",
                    tallies.aggro_average, tallies.aggro_peak
                ));
            });
        }
    }

    /// The performed events' recent past, newest last.
    fn event_log(&self, ui: &mut egui::Ui, snapshot: &UiSnapshot) {
        ui.heading("Events");
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                if snapshot.events.is_empty() {
                    ui.weak("no game events yet");
                    return;
                }
                for entry in &snapshot.events {
                    ui.label(format!("{:>9.2}s  {:?}", entry.when, entry.event));
                }
            });
    }
}

/// The chord's short name — the full entry is like "V7 (dominant seventh)".
fn short_name(name: &str) -> &str {
    name.split_once(" (").map_or(name, |(short, _)| short)
}

/// Sample triples to plot points, picking which coordinate is y.
fn samples_of(
    samples: &[(f64, f64, f64)],
    pick: impl Fn(&(f64, f64, f64)) -> (f64, f64),
) -> Vec<[f64; 2]> {
    samples.iter().map(|sample| {
        let (x, y) = pick(sample);
        [x, y]
    }).collect()
}

/// Control points spread evenly across the phrase parameter 0–1.
fn control_points(points: &[f64]) -> Vec<[f64; 2]> {
    if points.is_empty() {
        return Vec::new();
    }
    let span = (points.len() - 1).max(1) as f64;
    points
        .iter()
        .enumerate()
        .map(|(index, value)| [index as f64 / span, *value])
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum CellState {
    Past,
    Current,
    Future,
}

/// A planned chord slot in the strip.
fn chord_cell(ui: &mut egui::Ui, slot: &PlannedSlotView, state: CellState) {
    let (stroke, fill) = match state {
        CellState::Past => (egui::Stroke::new(1.0_f32, egui::Color32::DARK_GRAY), egui::Color32::BLACK),
        CellState::Current => (
            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(96, 220, 255)),
            egui::Color32::from_rgb(24, 40, 56),
        ),
        CellState::Future => (egui::Stroke::new(1.0_f32, egui::Color32::GRAY), egui::Color32::BLACK),
    };
    let label = match slot.bass {
        Some(tone) => format!("{}\n(inv {})", short_name(slot.name), tone),
        None => short_name(slot.name).to_string(),
    };
    egui::Frame::default()
        .stroke(stroke)
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_min_width(72.0);
            ui.label(egui::RichText::new(label).small());
        })
        .response
        .on_hover_text(format!("{} — {}", slot.name, slot.function));
}

/// A performed slot in the history — cut rings red, held rings blue.
fn history_cell(ui: &mut egui::Ui, record: &SlotRecord) {
    let name = VOCABULARY
        .get(record.chord as usize)
        .map_or("?", |entry| short_name(entry.name));
    let stroke = if record.cut {
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 96, 96))
    } else if record.held {
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(96, 160, 255))
    } else {
        egui::Stroke::new(1.0_f32, egui::Color32::GRAY)
    };
    egui::Frame::default()
        .stroke(stroke)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_min_width(72.0);
            ui.label(
                egui::RichText::new(format!("{name}\n{:.2}s", record.duration_secs)).small(),
            );
        })
        .response
        .on_hover_text({
            let mut tooltip = format!("{} — {:.2}s", VOCABULARY[record.chord as usize].name, record.duration_secs);
            if record.cut {
                tooltip.push_str(" — cut short by an aggro spike");
            }
            if record.held {
                tooltip.push_str(" — the rhythm held its rung");
            }
            tooltip
        });
}

/// One tallies grid row.
fn tally_row<T: std::fmt::Display>(ui: &mut egui::Ui, label: &str, value: T) {
    ui.label(label);
    ui.label(value.to_string());
    ui.end_row();
}
