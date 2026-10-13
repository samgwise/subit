//! First-run hints: a retiring sequence of control reminders — move, then
//! cleave, then shield — for a player the ability tray can't help yet
//! (every icon is dark until abilities are owned). Each hint names both
//! bindings and completes on the action itself, in any order; the last one
//! fades away for good. State is per-launch: no persistence.

use bevy::prelude::*;

use crate::input::{CLEAVE_PAD_BUTTONS, MoveIntent, SHIELD_PAD_BUTTONS};
use crate::skills::GameState;

/// Seconds the final line's fade-out runs for.
const FADE_SECS: f32 = 0.4;

/// The hint lines in teaching order — movement, then the two combat
/// verbs. Both bindings are named on every line: a pad player's aim may
/// not have claimed the device yet at step 1, so per-device labels are
/// the tray's job, not the hints'.
const HINTS: [&str; 3] = [
    "Move — WASD / left stick",
    "Cleave — left mouse / RT",
    "Shield — right mouse / LT",
];

/// The current hint: the first step not yet performed, or `None` when the
/// sequence is done — an early signal (a cleave while the move hint still
/// shows) skips the list ahead.
fn current_step(performed: [bool; 3]) -> Option<usize> {
    performed.iter().position(|done| !done)
}

/// Alpha over a fade tail: full until `duration − FADE_SECS` has elapsed,
/// then a linear fall to zero.
fn hint_alpha(elapsed: f32, duration: f32) -> f32 {
    ((duration - elapsed) / FADE_SECS).clamp(0.0, 1.0)
}

/// The hint line's text node.
#[derive(Component)]
struct HintLine;

pub struct HintsPlugin;

impl Plugin for HintsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hints)
            .add_systems(Update, sync_hints.run_if(in_state(GameState::Playing)));
    }
}

/// Spawn the bottom-centre hint line.
fn spawn_hints(mut commands: Commands) {
    commands
        .spawn((Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(16.0),
            left: Val::Px(0.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },))
        .with_children(|banner| {
            banner.spawn((
                HintLine,
                Text::new(HINTS[0]),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.9, 1.0, 0.9)),
            ));
        });
}

/// Advance the sequence on the player's actions and fade the line away
/// once every step is done.
#[allow(clippy::type_complexity)]
fn sync_hints(
    intent: Res<MoveIntent>,
    mouse: Res<ButtonInput<MouseButton>>,
    pads: Query<&Gamepad>,
    time: Res<Time>,
    mut performed: Local<[bool; 3]>,
    mut fading: Local<Option<Timer>>,
    line: Single<(&mut Text, &mut TextColor), With<HintLine>>,
) {
    let (mut text, mut colour) = line.into_inner();

    // Record what this frame's actions prove.
    if intent.0 != Vec2::ZERO {
        performed[0] = true;
    }
    if mouse.just_pressed(MouseButton::Left)
        || crate::input::just_pressed(&pads, CLEAVE_PAD_BUTTONS)
    {
        performed[1] = true;
    }
    if mouse.just_pressed(MouseButton::Right)
        || crate::input::just_pressed(&pads, SHIELD_PAD_BUTTONS)
    {
        performed[2] = true;
    }

    // While steps remain, the line names the first one.
    let Some(step) = current_step(*performed) else {
        // Every step done: fade the final line away, then hold it hidden.
        let timer = fading.get_or_insert_with(|| Timer::from_seconds(FADE_SECS, TimerMode::Once));
        timer.tick(time.delta());
        if timer.is_finished() {
            text.0 = String::new();
            colour.0.set_alpha(0.0);
        } else {
            let alpha = hint_alpha(timer.elapsed_secs(), FADE_SECS);
            colour.0.set_alpha(alpha);
        }
        return;
    };
    if text.0 != HINTS[step] {
        *text = Text::new(HINTS[step]);
        colour.0.set_alpha(0.9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sequence_walks_the_first_incomplete_step() {
        assert_eq!(current_step([false, false, false]), Some(0));
        assert_eq!(current_step([true, false, false]), Some(1));
        assert_eq!(current_step([true, true, false]), Some(2));
        // An early signal doesn't clear earlier steps: a cleave while the
        // move hint still shows leaves the move hint standing until the
        // player actually moves.
        assert_eq!(current_step([false, true, false]), Some(0));
        assert_eq!(current_step([true, true, true]), None);
    }

    #[test]
    fn every_hint_names_both_bindings_in_teaching_order() {
        for hint in HINTS {
            assert!(hint.contains('/'), "both bindings on: {hint}");
        }
        assert!(HINTS[0].starts_with("Move"));
        assert!(HINTS[1].starts_with("Cleave"));
        assert!(HINTS[2].starts_with("Shield"));
    }

    #[test]
    fn the_final_line_fades_over_its_tail() {
        assert_eq!(hint_alpha(0.0, FADE_SECS), 1.0);
        assert!((hint_alpha(FADE_SECS / 2.0, FADE_SECS) - 0.5).abs() < 1e-6);
        assert_eq!(hint_alpha(FADE_SECS, FADE_SECS), 0.0);
    }
}
