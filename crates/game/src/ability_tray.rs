//! Ability tray: the bottom-right row of ability icons — locked, ready or
//! cooling under a draining veil with a seconds countdown — plus the unlock
//! toast that announces a fresh purchase on return to combat. Keybind
//! labels follow the live device (keyboard+mouse or pad), and the whole
//! tray freezes with the world while the skills menu is open, mirroring
//! the timers' own cadence.

use bevy::prelude::*;

use crate::combat::{CooldownState, NovaState, PlayerShield};
use crate::input::Aim;
use crate::projectiles::GrenadeCooldown;
use crate::skills::{AbilityUnlocks, CloakState, DashState, GameState};

/// Side of one ability icon, in pixels.
const ICON_SIZE: f32 = 40.0;
/// Gap between icons, in pixels.
const ICON_GAP: f32 = 8.0;
/// Border thickness of an icon, in pixels.
const ICON_BORDER: f32 = 2.0;
/// Font size of an icon's label, in pixels.
const ICON_FONT: f32 = 12.0;
/// Seconds an unlock toast stays on screen, fading over its tail.
const TOAST_SECS: f32 = 2.5;
/// The toast's fade-out tail, in seconds.
const TOAST_FADE_SECS: f32 = 0.5;

/// One tray ability: its name, the keybind of each device and the tint of
/// its border. Cleave has no icon (its 0.25 s cooldown is noise) and the
/// passives (barrier, transmitter, volley) have no keybind to teach — the
/// barrier bar and the toast cover them.
struct TrayAbility {
    name: &'static str,
    keyboard: &'static str,
    pad: &'static str,
    tint: Color,
}

/// The tray's abilities in display order: shield, dash, grenade, nova,
/// cloak.
const TRAY_ABILITIES: [TrayAbility; 5] = [
    TrayAbility {
        name: "SHIELD",
        keyboard: "RMB",
        pad: "LT",
        tint: Color::srgb(0.4, 0.9, 1.0),
    },
    TrayAbility {
        name: "DASH",
        keyboard: "SPACE",
        pad: "LB",
        tint: Color::srgb(0.85, 0.95, 1.0),
    },
    TrayAbility {
        name: "GRENADE",
        keyboard: "G",
        pad: "RB",
        tint: Color::srgb(1.0, 0.62, 0.15),
    },
    TrayAbility {
        name: "NOVA",
        keyboard: "E",
        pad: "Y",
        tint: Color::srgb(0.7, 0.95, 1.0),
    },
    TrayAbility {
        name: "CLOAK",
        keyboard: "C",
        pad: "L3",
        tint: Color::srgb(0.55, 0.4, 1.0),
    },
];

/// The tray indices of the abilities an unlock purchase can grant (the
/// shield is always owned) — the toast's mirror walks these.
const UNLOCKABLES: [usize; 4] = [1, 2, 3, 4];

/// What an icon should show this frame, derived from its ability's timers.
struct IconState {
    unlocked: bool,
    /// Fraction of the cooldown still to run: 1.0 just used, 0.0 ready.
    remaining: f32,
    /// Seconds left on the cooldown (0.0 when ready).
    seconds: f32,
}

impl IconState {
    fn ready(&self) -> bool {
        self.unlocked && self.remaining <= 0.0
    }
}

impl From<CooldownState> for IconState {
    fn from(state: CooldownState) -> Self {
        Self {
            unlocked: false,
            remaining: state.remaining,
            seconds: state.seconds,
        }
    }
}

/// Border colour for an icon's state: grey while locked, the ability's
/// tint dimmed while cooling, full tint when ready.
fn border_colour(state: &IconState, tint: Color) -> Color {
    if !state.unlocked {
        return Color::srgba(0.35, 0.35, 0.4, 0.6);
    }
    if state.ready() {
        return tint;
    }
    let c = tint.to_srgba();
    Color::srgba(c.red * 0.45, c.green * 0.45, c.blue * 0.45, 0.8)
}

/// The centred label: the countdown while cooling, the keybind when ready,
/// nothing while locked — "2.4" tells the player exactly when the ability
/// wraps up.
fn icon_label(state: &IconState, key: &str) -> String {
    if !state.unlocked {
        return String::new();
    }
    match seconds_label(state.seconds) {
        Some(text) => text,
        None => key.to_string(),
    }
}

/// The cooldown countdown as a one-decimal string, or `None` when ready.
fn seconds_label(seconds: f32) -> Option<String> {
    (seconds > 0.0).then(|| format!("{seconds:.1}"))
}

/// Height percent of the cooldown veil: the full cover just after use,
/// draining to nothing as the timer elapses. An overshooting fraction
/// clamps — a stale read never inverts the veil.
fn overlay_height(remaining: f32) -> f32 {
    remaining.clamp(0.0, 1.0) * 100.0
}

/// Alpha of the toast text over its lifetime: full through the hold, then
/// a linear fall across the fade tail.
fn toast_alpha(elapsed: f32, duration: f32) -> f32 {
    ((duration - elapsed) / TOAST_FADE_SECS).clamp(0.0, 1.0)
}

/// The toast line for a fresh unlock, keyed for the live device.
fn toast_text(ability: &TrayAbility, pad: bool) -> String {
    let key = if pad { ability.pad } else { ability.keyboard };
    format!("{} UNLOCKED — {key}", ability.name)
}

/// Whether the tray ability at `index` is currently unlocked. Only the
/// unlock-gated abilities are ever asked (the shield is always owned).
fn unlocked(unlocks: &AbilityUnlocks, index: usize) -> bool {
    match index {
        1 => unlocks.dash,
        2 => unlocks.grenade,
        3 => unlocks.nova,
        4 => unlocks.cloak,
        _ => false,
    }
}

/// One tray icon's root node, carrying its index into [`TRAY_ABILITIES`].
#[derive(Component)]
struct AbilityIcon(usize);

/// The dark cooldown veil inside an icon.
#[derive(Component)]
struct IconOverlay;

/// The centred keybind/countdown label of an icon.
#[derive(Component)]
struct IconText;

/// The unlock toast's banner root.
#[derive(Component)]
struct ToastRoot;

/// The unlock toast's text.
#[derive(Component)]
struct Toast;

pub struct AbilityTrayPlugin;

impl Plugin for AbilityTrayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_tray).add_systems(
            Update,
            (sync_tray, sync_toast).run_if(in_state(GameState::Playing)),
        );
    }
}

/// Spawn the icon row and the hidden toast banner.
fn spawn_tray(mut commands: Commands) {
    commands
        .spawn((Node {
            position_type: PositionType::Absolute,
            right: Val::Px(16.0),
            bottom: Val::Px(16.0),
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(ICON_GAP),
            ..default()
        },))
        .with_children(|tray| {
            for i in 0..TRAY_ABILITIES.len() {
                tray.spawn((
                    AbilityIcon(i),
                    Node {
                        width: Val::Px(ICON_SIZE),
                        height: Val::Px(ICON_SIZE),
                        border: UiRect::all(Val::Px(ICON_BORDER)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                    BorderColor::all(Color::srgba(0.35, 0.35, 0.4, 0.6)),
                ))
                .with_children(|icon| {
                    // The veil renders under the label (first child).
                    icon.spawn((
                        IconOverlay,
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(0.0),
                            left: Val::Px(0.0),
                            width: Val::Percent(100.0),
                            height: Val::Percent(0.0),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
                    ));
                    icon.spawn((
                        IconText,
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(ICON_FONT),
                            ..default()
                        },
                        TextColor(Color::srgba(0.8, 0.9, 1.0, 0.9)),
                    ));
                });
            }
        });

    commands
        .spawn((
            ToastRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|banner| {
            banner.spawn((
                Toast,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.9, 1.0, 1.0)),
            ));
        });
}

/// Bind every icon's border, veil and label to its ability's timers.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn sync_tray(
    unlocks: Res<AbilityUnlocks>,
    shield: Res<PlayerShield>,
    dash: Res<DashState>,
    cloak: Res<CloakState>,
    nova: Res<NovaState>,
    grenade: Res<GrenadeCooldown>,
    aim: Res<Aim>,
    mut icons: Query<(&AbilityIcon, &mut BorderColor, &Children)>,
    mut overlays: Query<&mut Node, With<IconOverlay>>,
    mut texts: Query<&mut Text, With<IconText>>,
) {
    let states = [
        IconState {
            unlocked: true,
            ..shield.cooldown_state().into()
        },
        IconState {
            unlocked: unlocks.dash,
            ..CooldownState::of(&dash.cooldown).into()
        },
        IconState {
            unlocked: unlocks.grenade,
            ..grenade.cooldown_state().into()
        },
        IconState {
            unlocked: unlocks.nova,
            ..nova.cooldown_state().into()
        },
        IconState {
            unlocked: unlocks.cloak,
            ..CooldownState::of(&cloak.cooldown).into()
        },
    ];
    for (icon, mut border, children) in &mut icons {
        let ability = &TRAY_ABILITIES[icon.0];
        let state = &states[icon.0];
        border.set_all(border_colour(state, ability.tint));
        // The label re-derives whenever the wanted string moves — a device
        // flip changes the keybind, the countdown swaps for the keybind as
        // the cooldown elapses.
        let key = if aim.is_pad() {
            ability.pad
        } else {
            ability.keyboard
        };
        let wanted = icon_label(state, key);
        for &child in children {
            if let Ok(mut node) = overlays.get_mut(child) {
                node.height = Val::Percent(overlay_height(state.remaining));
            }
            if let Ok(mut text) = texts.get_mut(child)
                && text.0 != wanted
            {
                text.0 = wanted.clone();
            }
        }
    }
}

/// Announce a fresh unlock on the first playing frame it surfaces, then
/// fade the line away. Purchases land while the menu holds the world; the
/// mirror diff catches up the moment combat resumes.
#[allow(clippy::type_complexity)]
fn sync_toast(
    unlocks: Res<AbilityUnlocks>,
    aim: Res<Aim>,
    time: Res<Time>,
    mut mirror: Local<[bool; 4]>,
    mut active: Local<Option<Timer>>,
    mut root: Single<&mut Visibility, With<ToastRoot>>,
    toast: Single<(&mut Text, &mut TextColor), With<Toast>>,
) {
    let (mut text, mut colour) = toast.into_inner();
    // Fresh unlocks: a purchase while paused surfaces here as a flip.
    let mut announce = None;
    for (slot, &index) in UNLOCKABLES.iter().enumerate() {
        let now = unlocked(&unlocks, index);
        if now && !mirror[slot] {
            announce = Some(index);
        }
        mirror[slot] = now;
    }
    if let Some(index) = announce {
        *text = Text::new(toast_text(&TRAY_ABILITIES[index], aim.is_pad()));
        colour.0.set_alpha(1.0);
        active.replace(Timer::from_seconds(TOAST_SECS, TimerMode::Once));
        root.set_if_neq(Visibility::Visible);
    }
    // Tick and fade whatever toast is showing.
    let Some(timer) = active.as_mut() else {
        root.set_if_neq(Visibility::Hidden);
        return;
    };
    timer.tick(time.delta());
    if timer.is_finished() {
        active.take();
        root.set_if_neq(Visibility::Hidden);
    } else {
        let alpha = toast_alpha(timer.elapsed_secs(), TOAST_SECS);
        colour.0.set_alpha(alpha);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINT: Color = Color::srgb(0.4, 0.9, 1.0);

    #[test]
    fn seconds_label_reads_one_decimal_then_hides() {
        assert_eq!(seconds_label(2.34).as_deref(), Some("2.3"));
        assert_eq!(seconds_label(0.2).as_deref(), Some("0.2"));
        // A rested timer reads zero: no countdown at all.
        assert_eq!(seconds_label(0.0), None);
        assert_eq!(seconds_label(-0.1), None);
    }

    #[test]
    fn the_label_holds_the_countdown_while_cooling_and_the_key_when_ready() {
        let mut state = IconState {
            unlocked: true,
            remaining: 1.0,
            seconds: 2.0,
        };
        assert_eq!(icon_label(&state, "SPACE"), "2.0");
        state.remaining = 0.0;
        state.seconds = 0.0;
        assert_eq!(icon_label(&state, "SPACE"), "SPACE");
        // Locked: nothing to say at all.
        state.unlocked = false;
        assert_eq!(icon_label(&state, "SPACE"), "");
    }

    #[test]
    fn borders_read_locked_dim_and_ready() {
        let locked = IconState {
            unlocked: false,
            remaining: 0.0,
            seconds: 0.0,
        };
        assert_eq!(
            border_colour(&locked, TINT),
            Color::srgba(0.35, 0.35, 0.4, 0.6)
        );
        let mut cooling = IconState {
            unlocked: true,
            remaining: 0.5,
            seconds: 1.0,
        };
        assert!(
            border_colour(&cooling, TINT) != TINT,
            "cooling dims the tint"
        );
        cooling.remaining = 0.0;
        assert_eq!(border_colour(&cooling, TINT), TINT);
    }

    #[test]
    fn the_veil_starts_full_and_drains_to_nothing() {
        assert_eq!(overlay_height(1.0), 100.0);
        assert_eq!(overlay_height(0.25), 25.0);
        assert_eq!(overlay_height(0.0), 0.0);
        // An overshooting fraction clamps — a stale read never inverts.
        assert_eq!(overlay_height(1.5), 100.0);
    }

    #[test]
    fn the_toast_fades_over_its_tail() {
        // Full through the hold...
        assert_eq!(toast_alpha(0.0, TOAST_SECS), 1.0);
        assert_eq!(toast_alpha(TOAST_SECS - TOAST_FADE_SECS, TOAST_SECS), 1.0);
        // ...linear through the fade...
        assert!((toast_alpha(TOAST_SECS - TOAST_FADE_SECS / 2.0, TOAST_SECS) - 0.5).abs() < 1e-6);
        // ...and gone at the end.
        assert_eq!(toast_alpha(TOAST_SECS, TOAST_SECS), 0.0);
    }

    #[test]
    fn the_toast_names_the_ability_and_its_device_key() {
        let ability = &TRAY_ABILITIES[1]; // dash
        assert_eq!(toast_text(ability, false), "DASH UNLOCKED — SPACE");
        assert_eq!(toast_text(ability, true), "DASH UNLOCKED — LB");
    }

    #[test]
    fn the_tray_reads_the_four_unlocks_by_index() {
        let unlocks = AbilityUnlocks {
            dash: true,
            grenade: false,
            nova: true,
            cloak: false,
            ..Default::default()
        };
        // The shield (index 0) is never asked; the unlockables report.
        for &index in UNLOCKABLES.iter() {
            let want = match index {
                1 => unlocks.dash,
                2 => unlocks.grenade,
                3 => unlocks.nova,
                4 => unlocks.cloak,
                _ => unreachable!("UNLOCKABLES only holds the four unlockables"),
            };
            assert_eq!(unlocked(&unlocks, index), want);
        }
    }
}
