//! The piano roll: the scheduled notes on pitch lanes over time, with
//! the CC lanes beneath. A direct painter job — the roll's note spans
//! are rects, which the plot crate's line-and-point items don't speak.

use eframe::egui;

use crate::midi_log::{channel_name, CcPoint, NoteSpan};
use crate::snapshot::UiSnapshot;

/// The visible window: seconds behind the playhead, and the glimpse of
/// the scheduled future (the quantiser never schedules in the past, so
/// the future edge is where the up-coming notes wait).
const PAST_SECS: f64 = 12.0;
const FUTURE_SECS: f64 = 2.0;

/// The pitch range's clamps when the visible notes want to wander
/// beyond four octaves.
const PITCH_FLOOR: f64 = 24.0;
const PITCH_CEILING: f64 = 96.0;

/// The roll's height and each CC lane's height.
const ROLL_HEIGHT: f32 = 150.0;
const CC_LANE_HEIGHT: f32 = 24.0;

/// The channels' colours — one hue each, shared by the legend and the
/// note spans.
fn channel_colour(channel: u8) -> egui::Color32 {
    match channel {
        crate::music::CHANNEL_BASS => egui::Color32::from_rgb(255, 160, 64),
        crate::music::CHANNEL_DISTANT => egui::Color32::from_rgb(96, 128, 255),
        crate::music::CHANNEL_BELL => egui::Color32::from_rgb(255, 220, 96),
        crate::music::CHANNEL_AIRY => egui::Color32::from_rgb(96, 220, 255),
        crate::music::CHANNEL_PATTERN_A => egui::Color32::from_rgb(96, 220, 128),
        crate::music::CHANNEL_PATTERN_B => egui::Color32::from_rgb(190, 120, 255),
        crate::FX_CHANNEL => egui::Color32::from_rgb(255, 96, 128),
        _ => egui::Color32::GRAY,
    }
}

/// A controller's lane name — the known ones speak, the rest count.
fn controller_name(controller: u8) -> String {
    match controller {
        1 => "wheel".into(),
        11 => "expr".into(),
        other => format!("CC{other}"),
    }
}

/// The pitch's name — C-1 style, the tooltips' word.
fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] =
        ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[usize::from(note) % 12], note as i32 / 12 - 1)
}

/// The roll panel: the legend, the pitch lanes, and the CC lanes
/// beneath — one shared time axis.
pub fn panel(ui: &mut egui::Ui, snapshot: &UiSnapshot) {
    let now = snapshot.clock.hub_time;

    legend(ui, snapshot);
    let (min_pitch, max_pitch) = pitch_range(snapshot, now);

    ui.add_space(2.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROLL_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);

    let window = (now - PAST_SECS, now + FUTURE_SECS);
    let time_to_x = |t: f64| {
        rect.left()
            + (((t - window.0) / (window.1 - window.0)) as f32) * rect.width()
    };
    let pitch_to_y = |pitch: f64| {
        rect.top()
            + (((max_pitch - pitch) / (max_pitch - min_pitch)) as f32) * rect.height()
    };

    paint_lanes(&painter, rect, min_pitch, max_pitch);
    paint_spans(&painter, snapshot.roll.iter(), now, window, &time_to_x, &pitch_to_y);
    paint_cc_lanes(ui, snapshot, window, &time_to_x);

    // The playhead rides over everything.
    let x = time_to_x(now);
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(1.5_f32, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)),
    );

    // The hovered span names itself.
    if let Some(pointer) = response.hover_pos()
        && let Some(span) = span_at(snapshot.roll.iter(), pointer, now, window, &time_to_x, &pitch_to_y)
    {
        let name = channel_name(span.channel).unwrap_or("?");
        let fate = if span.open { "key down" } else { "stopped" };
        response.on_hover_text(format!(
            "{} — {} ({name}, vel {}, {fate})",
            note_name(span.note),
            span.channel,
            span.velocity,
        ));
    }
}

/// The legend: every channel the log holds, named and coloured.
fn legend(ui: &mut egui::Ui, snapshot: &UiSnapshot) {
    ui.horizontal(|ui| {
        ui.weak("roll:");
        let mut seen: Vec<u8> = snapshot.roll.iter().map(|span| span.channel).collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.is_empty() {
            ui.weak("no notes scheduled yet");
            return;
        }
        for channel in seen {
            let name = channel_name(channel).unwrap_or("?");
            ui.label(egui::RichText::new("■").color(channel_colour(channel)));
            ui.label(name);
        }
    });
}

/// The visible pitch range: the notes on screen, padded a semitone each
/// side and clamped — a default octave pair when the roll is empty.
fn pitch_range(snapshot: &UiSnapshot, now: f64) -> (f64, f64) {
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for span in visible_spans(snapshot.roll.iter(), now) {
        lowest = lowest.min(f64::from(span.note));
        highest = highest.max(f64::from(span.note));
    }
    if !lowest.is_finite() {
        return (48.0, 72.0);
    }
    (
        (lowest - 2.0).max(PITCH_FLOOR),
        (highest + 2.0).min(PITCH_CEILING),
    )
}

/// The spans that overlap the visible window.
fn visible_spans<'a>(
    spans: impl Iterator<Item = &'a NoteSpan>,
    now: f64,
) -> impl Iterator<Item = &'a NoteSpan> {
    let (window_start, window_end) = (now - PAST_SECS, now + FUTURE_SECS);
    spans
        .filter(move |span| span.end > window_start && span.start < window_end)
}

/// The span under a pointer, if any.
fn span_at<'a>(
    spans: impl Iterator<Item = &'a NoteSpan>,
    pointer: egui::Pos2,
    now: f64,
    window: (f64, f64),
    time_to_x: &impl Fn(f64) -> f32,
    pitch_to_y: &impl Fn(f64) -> f32,
) -> Option<&'a NoteSpan> {
    visible_spans(spans, now).find(|span| {
        let x0 = time_to_x(span.start);
        let x1 = time_to_x(span.end.min(window.1));
        let y_centre = pitch_to_y(f64::from(span.note));
        pointer.x >= x0 && pointer.x <= x1 && (pointer.y - y_centre).abs() <= 6.0
    })
}

/// The semitone lanes: black keys shaded, C lanes lined and named.
fn paint_lanes(painter: &egui::Painter, rect: egui::Rect, min_pitch: f64, max_pitch: f64) {
    let lanes = (max_pitch - min_pitch).max(1.0);
    let lane_height = rect.height() / lanes as f32;
    let lane_top = |from_pitch: f64| {
        rect.top() + (((max_pitch - from_pitch) / lanes) as f32) * rect.height()
    };
    for pitch in (min_pitch as i32)..=(max_pitch as i32) {
        let pitch = pitch as f64;
        let top = lane_top(pitch);
        if matches!(pitch as i32 % 12, 1 | 3 | 6 | 8 | 10) {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), top),
                    egui::vec2(rect.width(), lane_height),
                ),
                0.0_f32,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 8),
            );
        }
        if pitch as i32 % 12 == 0 {
            // The C's rail and name.
            painter.line_segment(
                [egui::pos2(rect.left(), top), egui::pos2(rect.right(), top)],
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
            );
            painter.text(
                egui::pos2(rect.left() + 4.0, top + lane_height * 0.5),
                egui::Align2::LEFT_CENTER,
                format!("C{}", pitch as i32 / 12 - 1),
                egui::FontId::monospace(9.0),
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
            );
        }
    }
}

/// The note spans: filled when sounding or past, outlined when entirely
/// in the scheduled future; alpha carries the velocity.
fn paint_spans<'a>(
    painter: &egui::Painter,
    spans: impl Iterator<Item = &'a NoteSpan>,
    now: f64,
    window: (f64, f64),
    time_to_x: &impl Fn(f64) -> f32,
    pitch_to_y: &impl Fn(f64) -> f32,
) {
    for span in visible_spans(spans, now) {
        let colour = channel_colour(span.channel);
        let x0 = time_to_x(span.start.max(window.0));
        let x1 = time_to_x(span.end.min(window.1));
        if x1 - x0 < 1.0 {
            continue;
        }
        let y_centre = pitch_to_y(f64::from(span.note));
        let half_height = (3.5_f32).min((pitch_to_y(f64::from(span.note) - 1.0) - y_centre).abs());
        let rect = egui::Rect::from_min_size(
            egui::pos2(x0, y_centre - half_height),
            egui::vec2(x1 - x0, half_height * 2.0),
        );
        if span.start >= now {
            // Scheduled: an outline waiting its turn.
            painter.rect_stroke(
                rect,
                1.0_f32,
                egui::Stroke::new(1.0_f32, colour),
                egui::StrokeKind::Inside,
            );
        } else {
            let alpha = (60.0 + 170.0 * f32::from(span.velocity) / 127.0) as u8;
            painter.rect_filled(
                rect,
                1.0_f32,
                egui::Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha),
            );
        }
    }
}

/// The CC lanes beneath the roll: one per (channel, controller) pair
/// present, sharing the time axis.
fn paint_cc_lanes(
    ui: &mut egui::Ui,
    snapshot: &UiSnapshot,
    window: (f64, f64),
    time_to_x: &impl Fn(f64) -> f32,
) {
    let mut lanes: Vec<((u8, u8), Vec<&CcPoint>)> = Vec::new();
    for point in snapshot.ccs.iter().filter(|point| point.when >= window.0) {
        let key = (point.channel, point.controller);
        if let Some(lane) = lanes.iter_mut().find(|(lane_key, _)| *lane_key == key) {
            lane.1.push(point);
        } else {
            lanes.push((key, vec![point]));
        }
    }

    for ((channel, controller), mut points) in lanes {
        points.sort_by(|a, b| a.when.partial_cmp(&b.when).unwrap_or(std::cmp::Ordering::Equal));
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), CC_LANE_HEIGHT),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            0.0_f32,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 6),
        );
        let colour = channel_colour(channel);
        let y_at = |value: u8| rect.bottom() - (f32::from(value) / 127.0) * rect.height();
        let shape: Vec<egui::Pos2> = points
            .iter()
            .map(|point| egui::pos2(time_to_x(point.when), y_at(point.value)))
            .collect();
        if shape.len() > 1 {
            painter.add(egui::Shape::line(shape, egui::Stroke::new(1.5_f32, colour)));
        }
        let name = channel_name(channel).unwrap_or("?");
        painter.text(
            egui::pos2(rect.left() + 4.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{name} · {}", controller_name(controller)),
            egui::FontId::monospace(8.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 110),
        );
        // The lane's full-scale rail.
        painter.line_segment(
            [egui::pos2(rect.left(), rect.top()), egui::pos2(rect.right(), rect.top())],
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20)),
        );
    }
}
