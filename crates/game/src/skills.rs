//! Skills: the pause state for the menu, purchased parameter upgrades and
//! ability unlocks, and the menu UI itself. Tab pauses the world and opens
//! the panel; purchases apply immediately.

use bevy::prelude::*;

use crate::combat::PlayerShield;
use crate::progression::SkillPoints;

/// How the app divides its time: the live game, or the paused skills menu.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameState {
    #[default]
    Playing,
    Menu,
}

/// Owned parameter upgrades.
#[derive(Resource, Debug, Default)]
pub struct SkillLevels {
    pub cleave: u32,
    pub shield: u32,
}

/// Purchased ability unlocks.
#[derive(Resource, Debug, Default)]
pub struct AbilityUnlocks {
    pub dash: bool,
    pub grenade: bool,
}

// --- Skill parameters -----------------------------------------------------

/// Base cleave damage before upgrades.
const CLEAVE_DAMAGE_BASE: i32 = 100;
/// Damage added per purchased point.
const CLEAVE_DAMAGE_PER_POINT: i32 = 25;

/// Current cleave damage.
pub fn cleave_damage(levels: &SkillLevels) -> i32 {
    CLEAVE_DAMAGE_BASE + CLEAVE_DAMAGE_PER_POINT * levels.cleave as i32
}

/// Shield cooldown before upgrades.
const SHIELD_COOLDOWN_BASE: f32 = 3.0;
/// Cooldown reduction per purchased point.
const SHIELD_COOLDOWN_PER_POINT: f32 = 0.25;
/// The cooldown never drops below this.
const SHIELD_COOLDOWN_MIN: f32 = 1.0;

/// Current shield cooldown in seconds.
pub fn shield_cooldown(levels: &SkillLevels) -> f32 {
    (SHIELD_COOLDOWN_BASE - SHIELD_COOLDOWN_PER_POINT * levels.shield as f32)
        .max(SHIELD_COOLDOWN_MIN)
}

// --- Costs -----------------------------------------------------------------

pub const SKILL_COST: u32 = 1;
pub const DASH_UNLOCK_COST: u32 = 2;
pub const GRENADE_UNLOCK_COST: u32 = 3;

// --- Dash state (the ability itself lives with movement) -------------------

/// Dash ability state, shared between the trigger system and movement.
#[derive(Resource, Debug, Default)]
pub struct DashState {
    pub active: Timer,
    pub cooldown: Timer,
    pub dir: Vec2,
}

/// Dash speed in world units per second.
pub const DASH_SPEED: f32 = 600.0;
/// Seconds of dash travel.
pub const DASH_SECS: f32 = 0.15;
/// Seconds between dashes.
pub const DASH_COOLDOWN_SECS: f32 = 2.0;

pub struct SkillsPlugin;

impl Plugin for SkillsPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<SkillLevels>()
            .init_resource::<AbilityUnlocks>()
            .insert_resource(DashState {
                active: Timer::from_seconds(DASH_SECS, TimerMode::Once),
                cooldown: Timer::from_seconds(DASH_COOLDOWN_SECS, TimerMode::Once),
                dir: Vec2::ZERO,
            })
            .add_systems(Startup, spawn_menu)
            .add_systems(Update, (toggle_menu, refresh_menu, handle_purchases));
    }
}

fn toggle_menu(
    input: Res<ButtonInput<KeyCode>>,
    state: Res<State<GameState>>,
    mut next: ResMut<NextState<GameState>>,
) {
    if input.just_pressed(KeyCode::Tab) {
        next.set(match state.get() {
            GameState::Playing => GameState::Menu,
            GameState::Menu => GameState::Playing,
        });
    }
}

/// Menu panel root, hidden while playing.
#[derive(Component)]
struct MenuRoot;

/// One purchasable row in the menu.
#[derive(Component, Debug, Clone, Copy)]
enum SkillRow {
    Cleave,
    Shield,
    Dash,
    Grenade,
}

impl SkillRow {
    fn label(&self) -> String {
        match self {
            SkillRow::Cleave => {
                format!("Cleave damage +{CLEAVE_DAMAGE_PER_POINT}  (cost {SKILL_COST})")
            }
            SkillRow::Shield => {
                format!("Shield cooldown -{SHIELD_COOLDOWN_PER_POINT}s  (cost {SKILL_COST})")
            }
            SkillRow::Dash => format!("Unlock dash  [Space]  (cost {DASH_UNLOCK_COST})"),
            SkillRow::Grenade => format!("Unlock grenade  [G]  (cost {GRENADE_UNLOCK_COST})"),
        }
    }

    /// Whether the row can be bought right now.
    fn available(&self, points: u32, _levels: &SkillLevels, unlocks: &AbilityUnlocks) -> bool {
        match self {
            SkillRow::Cleave | SkillRow::Shield => points >= SKILL_COST,
            SkillRow::Dash => !unlocks.dash && points >= DASH_UNLOCK_COST,
            SkillRow::Grenade => !unlocks.grenade && points >= GRENADE_UNLOCK_COST,
        }
    }
}

fn spawn_menu(mut commands: Commands) {
    commands
        .spawn((
            MenuRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    padding: UiRect::all(Val::Px(20.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.05, 0.05, 0.1, 0.95)),
            ))
            .with_children(|panel| {
                panel.spawn(Node::default()).with_children(|header| {
                    header.spawn(Text::new("SIGNAL BREACH — SKILLS (Tab to resume)"));
                });
                for row in [
                    SkillRow::Cleave,
                    SkillRow::Shield,
                    SkillRow::Dash,
                    SkillRow::Grenade,
                ] {
                    panel
                        .spawn((
                            row,
                            Button,
                            Node {
                                padding: UiRect::all(Val::Px(10.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.1, 0.2, 0.3, 0.9)),
                        ))
                        .with_children(|button| {
                            button.spawn(Text::new(row.label()));
                        });
                }
            });
        });
}

/// Buy a skill when its row is pressed and affordable.
fn handle_purchases(
    rows: Query<(&Interaction, &SkillRow), Changed<Interaction>>,
    mut points: ResMut<SkillPoints>,
    mut levels: ResMut<SkillLevels>,
    mut unlocks: ResMut<AbilityUnlocks>,
    mut shield: ResMut<PlayerShield>,
) {
    for (interaction, row) in &rows {
        if *interaction != Interaction::Pressed || !row.available(points.0, &levels, &unlocks) {
            continue;
        }
        match row {
            SkillRow::Cleave => {
                points.0 -= SKILL_COST;
                levels.cleave += 1;
                tracing::info!(cleave = levels.cleave, "purchased cleave damage");
            }
            SkillRow::Shield => {
                points.0 -= SKILL_COST;
                levels.shield += 1;
                // Apply immediately: shorten the live cooldown timer.
                shield.set_cooldown_secs(shield_cooldown(&levels));
                tracing::info!(shield = levels.shield, "purchased shield cooldown");
            }
            SkillRow::Dash => {
                points.0 -= DASH_UNLOCK_COST;
                unlocks.dash = true;
                tracing::info!("unlocked dash");
            }
            SkillRow::Grenade => {
                points.0 -= GRENADE_UNLOCK_COST;
                unlocks.grenade = true;
                tracing::info!("unlocked grenade");
            }
        }
    }
}

/// Keep menu visibility and row colours in sync with state and affordability.
fn refresh_menu(
    state: Res<State<GameState>>,
    points: Res<SkillPoints>,
    levels: Res<SkillLevels>,
    unlocks: Res<AbilityUnlocks>,
    mut root: Single<&mut Visibility, With<MenuRoot>>,
    mut rows: Query<(&SkillRow, &mut BackgroundColor)>,
) {
    root.set_if_neq(match state.get() {
        GameState::Playing => Visibility::Hidden,
        GameState::Menu => Visibility::Visible,
    });
    for (row, mut colour) in &mut rows {
        colour.0 = if row.available(points.0, &levels, &unlocks) {
            Color::srgba(0.1, 0.3, 0.45, 0.9)
        } else {
            Color::srgba(0.15, 0.15, 0.18, 0.9)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleave_damage_grows_and_shield_cooldown_shrinks_to_a_floor() {
        let mut levels = SkillLevels::default();
        assert_eq!(cleave_damage(&levels), CLEAVE_DAMAGE_BASE);
        levels.cleave = 2;
        assert_eq!(
            cleave_damage(&levels),
            CLEAVE_DAMAGE_BASE + 2 * CLEAVE_DAMAGE_PER_POINT
        );

        levels.shield = 0;
        assert!((shield_cooldown(&levels) - SHIELD_COOLDOWN_BASE).abs() < 1e-5);
        levels.shield = 4;
        assert!(
            (shield_cooldown(&levels) - (SHIELD_COOLDOWN_BASE - 4.0 * SHIELD_COOLDOWN_PER_POINT))
                .abs()
                < 1e-5
        );
        // Past the floor the cooldown stops shrinking.
        levels.shield = 100;
        assert!((shield_cooldown(&levels) - SHIELD_COOLDOWN_MIN).abs() < 1e-5);
    }

    #[test]
    fn rows_grey_out_on_cost_and_ownership() {
        let levels = SkillLevels::default();
        let mut unlocks = AbilityUnlocks::default();
        let empty = SkillRow::Cleave.available(0, &levels, &unlocks);
        assert!(!empty);
        assert!(SkillRow::Cleave.available(1, &levels, &unlocks));
        assert!(!SkillRow::Dash.available(1, &levels, &unlocks));
        assert!(SkillRow::Dash.available(2, &levels, &unlocks));
        unlocks.dash = true;
        assert!(!SkillRow::Dash.available(99, &levels, &unlocks));
        assert!(SkillRow::Grenade.available(3, &levels, &unlocks));
    }
}
