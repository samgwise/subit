//! Skills: the pause state for the menu, purchased parameter upgrades and
//! ability unlocks, and the menu UI itself. Tab pauses the world and opens
//! the panel; purchases apply immediately.

use std::time::Duration;

use bevy::prelude::*;

use crate::combat::{BARRIER_PLATES, BASE_MAX_HP, Barrier, PlayerShield};
use crate::progression::SkillPoints;
use crate::world::TILE_SIZE;

/// How the app divides its time: the live game, or the paused skills menu.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameState {
    #[default]
    Playing,
    Menu,
}

/// Owned parameter upgrades. Every repeatable skill levels here and its
/// effect reads through one pure function below.
#[derive(Resource, Debug, Default)]
pub struct SkillLevels {
    pub cleave: u32,
    pub cleave_reach: u32,
    pub shield: u32,
    pub shield_duration: u32,
    pub vitality: u32,
    pub dash: u32,
    pub grenade: u32,
}

/// Purchased ability unlocks.
#[derive(Resource, Debug, Default)]
pub struct AbilityUnlocks {
    pub dash: bool,
    pub grenade: bool,
    pub barrier: bool,
    pub nova: bool,
    pub deflect_volley: bool,
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

/// Cleave reach before upgrades — three tiles.
const CLEAVE_RADIUS_BASE_TILES: f32 = 3.0;
/// Reach added per purchased point (a quarter tile).
const CLEAVE_RADIUS_PER_POINT_TILES: f32 = 0.25;

/// Current cleave reach in world units.
pub fn cleave_radius(levels: &SkillLevels) -> f32 {
    TILE_SIZE
        * (CLEAVE_RADIUS_BASE_TILES + CLEAVE_RADIUS_PER_POINT_TILES * levels.cleave_reach as f32)
}

/// Half the base cleave arc — a 60-degree cone to begin with.
const CLEAVE_HALF_ANGLE_BASE: f32 = core::f32::consts::PI / 6.0;
/// Full-arc widening per purchased point, halved for the half-angle
/// (the full arc grows 5 degrees).
const CLEAVE_HALF_ANGLE_PER_POINT: f32 = core::f32::consts::PI / 72.0;

/// Current cleave half-arc in radians.
pub fn cleave_half_angle(levels: &SkillLevels) -> f32 {
    CLEAVE_HALF_ANGLE_BASE + CLEAVE_HALF_ANGLE_PER_POINT * levels.cleave_reach as f32
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

/// Shield duration before upgrades.
const SHIELD_ACTIVE_BASE: f32 = 0.6;
/// Duration added per purchased point.
const SHIELD_ACTIVE_PER_POINT: f32 = 0.15;

/// Current shield duration in seconds.
pub fn shield_duration_secs(levels: &SkillLevels) -> f32 {
    SHIELD_ACTIVE_BASE + SHIELD_ACTIVE_PER_POINT * levels.shield_duration as f32
}

/// Dash cooldown before upgrades.
const DASH_COOLDOWN_BASE: f32 = 2.0;
/// Cooldown reduction per purchased point.
const DASH_COOLDOWN_PER_POINT: f32 = 0.25;
/// The cooldown never drops below this.
const DASH_COOLDOWN_MIN: f32 = 1.0;

/// Current dash cooldown in seconds.
pub fn dash_cooldown_secs(levels: &SkillLevels) -> f32 {
    (DASH_COOLDOWN_BASE - DASH_COOLDOWN_PER_POINT * levels.dash as f32).max(DASH_COOLDOWN_MIN)
}

/// Grenade blast damage before upgrades.
const GRENADE_DAMAGE_BASE: i32 = 200;
/// Blast damage added per purchased point.
const GRENADE_DAMAGE_PER_POINT: i32 = 50;

/// Current grenade blast damage.
pub fn grenade_damage(levels: &SkillLevels) -> i32 {
    GRENADE_DAMAGE_BASE + GRENADE_DAMAGE_PER_POINT * levels.grenade as i32
}

/// Max health added per purchased vitality point.
const VITALITY_HP_PER_POINT: i32 = 25;

/// Current max health for the player's vitality investment.
pub fn max_hp_for(levels: &SkillLevels) -> i32 {
    BASE_MAX_HP + VITALITY_HP_PER_POINT * levels.vitality as i32
}

// --- Caps ------------------------------------------------------------------

/// Purchase caps for the repeatable skills that have them: the cooldown
/// floors, the shield's reflection window, the cleave's reach and the
/// grenade's blast ceiling.
pub const CLEAVE_REACH_CAP: u32 = 4;
pub const SHIELD_LEVEL_CAP: u32 = 4;
pub const SHIELD_DURATION_CAP: u32 = 4;
pub const DASH_LEVEL_CAP: u32 = 4;
pub const GRENADE_LEVEL_CAP: u32 = 6;

// --- Costs -----------------------------------------------------------------

pub const SKILL_COST: u32 = 1;
pub const DASH_UNLOCK_COST: u32 = 2;
pub const GRENADE_UNLOCK_COST: u32 = 3;
pub const BARRIER_UNLOCK_COST: u32 = 3;
pub const NOVA_UNLOCK_COST: u32 = 4;
pub const VOLLEY_UNLOCK_COST: u32 = 5;

// --- Dash state (the ability itself lives with movement) -------------------

/// Dash ability state, shared between the trigger system and movement.
#[derive(Resource, Debug, Default)]
pub struct DashState {
    pub active: Timer,
    pub cooldown: Timer,
    pub dir: Vec2,
}

impl DashState {
    /// Re-derive the cooldown timer after a menu purchase.
    pub fn set_cooldown_secs(&mut self, secs: f32) {
        self.cooldown
            .set_duration(Duration::from_secs_f64(secs as f64));
    }
}

/// Dash speed in world units per second.
pub const DASH_SPEED: f32 = 600.0;
/// Seconds of dash travel.
pub const DASH_SECS: f32 = 0.15;

pub struct SkillsPlugin;

impl Plugin for SkillsPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<SkillLevels>()
            .init_resource::<AbilityUnlocks>()
            .insert_resource(DashState {
                active: Timer::from_seconds(DASH_SECS, TimerMode::Once),
                cooldown: Timer::from_seconds(DASH_COOLDOWN_BASE, TimerMode::Once),
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
    CleaveReach,
    Shield,
    ShieldDuration,
    Vitality,
    Dash,
    DashCooldown,
    Grenade,
    GrenadeDamage,
    Nova,
    DeflectVolley,
    Barrier,
}

/// Menu text for a levelled row: effect, level out of the cap, and either
/// the cost or `maxed`.
fn levelled_label(name: &str, effect: &str, level: u32, cap: u32) -> String {
    let tail = if level >= cap {
        "maxed".to_string()
    } else {
        format!("cost {SKILL_COST}")
    };
    format!("{name} {effect}  (Lv {level}/{cap}, {tail})")
}

/// Menu text for an ability unlock row.
fn unlock_label(name: &str, key: Option<&str>, owned: bool, cost: u32) -> String {
    if owned {
        return format!("{name} unlocked");
    }
    match key {
        Some(key) => format!("Unlock {name}  [{key}]  (cost {cost})"),
        None => format!("Unlock {name}  (cost {cost})"),
    }
}

impl SkillRow {
    /// The cost of buying this row now, or `None` when it cannot be bought
    /// at all — maxed out, already owned, or gated behind its ability.
    fn cost(&self, levels: &SkillLevels, unlocks: &AbilityUnlocks) -> Option<u32> {
        match self {
            SkillRow::Cleave | SkillRow::Vitality => Some(SKILL_COST),
            SkillRow::CleaveReach => (levels.cleave_reach < CLEAVE_REACH_CAP).then_some(SKILL_COST),
            SkillRow::Shield => (levels.shield < SHIELD_LEVEL_CAP).then_some(SKILL_COST),
            SkillRow::ShieldDuration => {
                (levels.shield_duration < SHIELD_DURATION_CAP).then_some(SKILL_COST)
            }
            // The cooldown upgrades are dead weight before their ability.
            SkillRow::DashCooldown => {
                (unlocks.dash && levels.dash < DASH_LEVEL_CAP).then_some(SKILL_COST)
            }
            SkillRow::GrenadeDamage => {
                (unlocks.grenade && levels.grenade < GRENADE_LEVEL_CAP).then_some(SKILL_COST)
            }
            SkillRow::Dash => (!unlocks.dash).then_some(DASH_UNLOCK_COST),
            SkillRow::Grenade => (!unlocks.grenade).then_some(GRENADE_UNLOCK_COST),
            SkillRow::Nova => (!unlocks.nova).then_some(NOVA_UNLOCK_COST),
            SkillRow::DeflectVolley => (!unlocks.deflect_volley).then_some(VOLLEY_UNLOCK_COST),
            SkillRow::Barrier => (!unlocks.barrier).then_some(BARRIER_UNLOCK_COST),
        }
    }

    /// Whether the row can be bought right now: a cost exists and the
    /// points cover it.
    fn available(&self, points: u32, levels: &SkillLevels, unlocks: &AbilityUnlocks) -> bool {
        self.cost(levels, unlocks)
            .is_some_and(|cost| points >= cost)
    }

    /// Menu text, including the row's live level and cost.
    fn label(&self, levels: &SkillLevels, unlocks: &AbilityUnlocks) -> String {
        match self {
            SkillRow::Cleave => levelled_label(
                "Cleave damage",
                &format!("+{CLEAVE_DAMAGE_PER_POINT}"),
                levels.cleave,
                u32::MAX,
            ),
            SkillRow::CleaveReach => levelled_label(
                "Cleave reach",
                "+0.25 tiles, arc +5°",
                levels.cleave_reach,
                CLEAVE_REACH_CAP,
            ),
            SkillRow::Shield => levelled_label(
                "Shield cooldown",
                &format!("-{SHIELD_COOLDOWN_PER_POINT}s"),
                levels.shield,
                SHIELD_LEVEL_CAP,
            ),
            SkillRow::ShieldDuration => levelled_label(
                "Shield duration",
                &format!("+{SHIELD_ACTIVE_PER_POINT}s"),
                levels.shield_duration,
                SHIELD_DURATION_CAP,
            ),
            SkillRow::Vitality => levelled_label(
                "Max health",
                &format!("+{VITALITY_HP_PER_POINT}"),
                levels.vitality,
                u32::MAX,
            ),
            SkillRow::DashCooldown => {
                if !unlocks.dash {
                    "Dash cooldown −0.25s  (needs dash)".to_string()
                } else {
                    levelled_label(
                        "Dash cooldown",
                        &format!("-{DASH_COOLDOWN_PER_POINT}s"),
                        levels.dash,
                        DASH_LEVEL_CAP,
                    )
                }
            }
            SkillRow::GrenadeDamage => {
                if !unlocks.grenade {
                    "Grenade damage +50  (needs grenade)".to_string()
                } else {
                    levelled_label(
                        "Grenade damage",
                        &format!("+{GRENADE_DAMAGE_PER_POINT}"),
                        levels.grenade,
                        GRENADE_LEVEL_CAP,
                    )
                }
            }
            SkillRow::Dash => unlock_label("dash", Some("Space"), unlocks.dash, DASH_UNLOCK_COST),
            SkillRow::Grenade => {
                unlock_label("grenade", Some("G"), unlocks.grenade, GRENADE_UNLOCK_COST)
            }
            SkillRow::Nova => unlock_label("nova", Some("E"), unlocks.nova, NOVA_UNLOCK_COST),
            SkillRow::DeflectVolley => unlock_label(
                "deflect volley",
                None,
                unlocks.deflect_volley,
                VOLLEY_UNLOCK_COST,
            ),
            SkillRow::Barrier => {
                unlock_label("barrier", None, unlocks.barrier, BARRIER_UNLOCK_COST)
            }
        }
    }
}

/// The menu's rows in display order: upgrades first, then the unlocks.
const MENU_ROWS: [SkillRow; 12] = [
    SkillRow::Cleave,
    SkillRow::CleaveReach,
    SkillRow::Shield,
    SkillRow::ShieldDuration,
    SkillRow::Vitality,
    SkillRow::DashCooldown,
    SkillRow::GrenadeDamage,
    SkillRow::Dash,
    SkillRow::Grenade,
    SkillRow::Nova,
    SkillRow::DeflectVolley,
    SkillRow::Barrier,
];

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
                for row in MENU_ROWS {
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
                            button.spawn(Text::new(
                                row.label(&SkillLevels::default(), &AbilityUnlocks::default()),
                            ));
                        });
                }
            });
        });
}

/// Buy a skill when its row is pressed and affordable.
#[allow(clippy::too_many_arguments)]
fn handle_purchases(
    rows: Query<(&Interaction, &SkillRow), Changed<Interaction>>,
    mut points: ResMut<SkillPoints>,
    mut levels: ResMut<SkillLevels>,
    mut unlocks: ResMut<AbilityUnlocks>,
    mut shield: ResMut<PlayerShield>,
    mut dash: ResMut<DashState>,
    mut vitals: ResMut<crate::combat::PlayerVitals>,
    mut barrier: ResMut<Barrier>,
) {
    for (interaction, row) in &rows {
        let Some(cost) = row.cost(&levels, &unlocks) else {
            continue;
        };
        if *interaction != Interaction::Pressed || points.0 < cost {
            continue;
        }
        match row {
            SkillRow::Cleave => {
                points.0 -= SKILL_COST;
                levels.cleave += 1;
                tracing::info!(cleave = levels.cleave, "purchased cleave damage");
            }
            SkillRow::CleaveReach => {
                points.0 -= SKILL_COST;
                levels.cleave_reach += 1;
                tracing::info!(cleave_reach = levels.cleave_reach, "purchased cleave reach");
            }
            SkillRow::Shield => {
                points.0 -= SKILL_COST;
                levels.shield += 1;
                // Apply immediately: shorten the live cooldown timer.
                shield.set_cooldown_secs(shield_cooldown(&levels));
                tracing::info!(shield = levels.shield, "purchased shield cooldown");
            }
            SkillRow::ShieldDuration => {
                points.0 -= SKILL_COST;
                levels.shield_duration += 1;
                // Apply immediately: lengthen the live active timer.
                shield.set_active_secs(shield_duration_secs(&levels));
                tracing::info!(
                    shield_duration = levels.shield_duration,
                    "purchased shield duration"
                );
            }
            SkillRow::Vitality => {
                points.0 -= SKILL_COST;
                levels.vitality += 1;
                vitals.max_hp = max_hp_for(&levels);
                // The upgrade heals by the same amount it raises the ceiling.
                vitals.hp = (vitals.hp + VITALITY_HP_PER_POINT).min(vitals.max_hp);
                tracing::info!(vitality = levels.vitality, "purchased max health");
            }
            SkillRow::DashCooldown => {
                points.0 -= SKILL_COST;
                levels.dash += 1;
                // Apply immediately: shorten the live cooldown timer.
                dash.set_cooldown_secs(dash_cooldown_secs(&levels));
                tracing::info!(dash = levels.dash, "purchased dash cooldown");
            }
            SkillRow::GrenadeDamage => {
                points.0 -= SKILL_COST;
                levels.grenade += 1;
                tracing::info!(grenade = levels.grenade, "purchased grenade damage");
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
            SkillRow::Nova => {
                points.0 -= NOVA_UNLOCK_COST;
                unlocks.nova = true;
                tracing::info!("unlocked nova");
            }
            SkillRow::DeflectVolley => {
                points.0 -= VOLLEY_UNLOCK_COST;
                unlocks.deflect_volley = true;
                tracing::info!("unlocked deflect volley");
            }
            SkillRow::Barrier => {
                points.0 -= BARRIER_UNLOCK_COST;
                unlocks.barrier = true;
                // The purchase fills the pool; regen keeps it topped up.
                barrier.plates = BARRIER_PLATES;
                tracing::info!("unlocked barrier");
            }
        }
    }
}

/// Keep menu visibility, row colours and row labels in sync with state and
/// affordability.
fn refresh_menu(
    state: Res<State<GameState>>,
    points: Res<SkillPoints>,
    levels: Res<SkillLevels>,
    unlocks: Res<AbilityUnlocks>,
    mut root: Single<&mut Visibility, With<MenuRoot>>,
    mut rows: Query<(&SkillRow, &Children, &mut BackgroundColor)>,
    mut texts: Query<&mut Text>,
) {
    root.set_if_neq(match state.get() {
        GameState::Playing => Visibility::Hidden,
        GameState::Menu => Visibility::Visible,
    });
    for (row, children, mut colour) in &mut rows {
        colour.0 = if row.available(points.0, &levels, &unlocks) {
            Color::srgba(0.1, 0.3, 0.45, 0.9)
        } else {
            Color::srgba(0.15, 0.15, 0.18, 0.9)
        };
        if let Some(child) = children.first()
            && let Ok(mut text) = texts.get_mut(*child)
        {
            *text = Text::new(row.label(&levels, &unlocks));
        }
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
    fn max_health_grows_with_vitality() {
        let mut levels = SkillLevels::default();
        assert_eq!(max_hp_for(&levels), BASE_MAX_HP);
        levels.vitality = 3;
        assert_eq!(max_hp_for(&levels), BASE_MAX_HP + 3 * VITALITY_HP_PER_POINT);
    }

    #[test]
    fn cleave_reach_extends_radius_and_widens_the_arc() {
        let mut levels = SkillLevels::default();
        // The base cone starts at 60 degrees (a 30-degree half).
        assert!((cleave_radius(&levels) - TILE_SIZE * 3.0).abs() < 1e-5);
        assert!((cleave_half_angle(&levels) - 30.0_f32.to_radians()).abs() < 1e-5);
        levels.cleave_reach = 4;
        assert!((cleave_radius(&levels) - TILE_SIZE * 4.0).abs() < 1e-5);
        assert!((cleave_half_angle(&levels) - 40.0_f32.to_radians()).abs() < 1e-5);
    }

    #[test]
    fn shield_duration_grows_and_dash_cooldown_shrinks_to_a_floor() {
        let mut levels = SkillLevels::default();
        assert!((shield_duration_secs(&levels) - 0.6).abs() < 1e-5);
        levels.shield_duration = 4;
        assert!((shield_duration_secs(&levels) - 1.2).abs() < 1e-5);

        levels.dash = 0;
        assert!((dash_cooldown_secs(&levels) - 2.0).abs() < 1e-5);
        levels.dash = 4;
        assert!((dash_cooldown_secs(&levels) - 1.0).abs() < 1e-5);
        // Past the floor the cooldown stops shrinking.
        levels.dash = 100;
        assert!((dash_cooldown_secs(&levels) - DASH_COOLDOWN_MIN).abs() < 1e-5);
    }

    #[test]
    fn grenade_damage_grows() {
        let mut levels = SkillLevels::default();
        assert_eq!(grenade_damage(&levels), 200);
        levels.grenade = 6;
        assert_eq!(grenade_damage(&levels), 500);
    }

    #[test]
    fn costs_respect_caps_and_prerequisites() {
        let mut levels = SkillLevels::default();
        let mut unlocks = AbilityUnlocks::default();

        // Unlocked repeatables always cost the flat rate.
        assert_eq!(SkillRow::Cleave.cost(&levels, &unlocks), Some(SKILL_COST));
        assert_eq!(
            SkillRow::CleaveReach.cost(&levels, &unlocks),
            Some(SKILL_COST)
        );

        // Capped rows dry up at the cap.
        levels.cleave_reach = CLEAVE_REACH_CAP;
        assert_eq!(SkillRow::CleaveReach.cost(&levels, &unlocks), None);

        // The cooldown upgrades need their ability first.
        assert_eq!(SkillRow::DashCooldown.cost(&levels, &unlocks), None);
        unlocks.dash = true;
        assert_eq!(
            SkillRow::DashCooldown.cost(&levels, &unlocks),
            Some(SKILL_COST)
        );
        levels.dash = DASH_LEVEL_CAP;
        assert_eq!(SkillRow::DashCooldown.cost(&levels, &unlocks), None);

        assert_eq!(SkillRow::GrenadeDamage.cost(&levels, &unlocks), None);
        unlocks.grenade = true;
        assert_eq!(
            SkillRow::GrenadeDamage.cost(&levels, &unlocks),
            Some(SKILL_COST)
        );
        levels.grenade = GRENADE_LEVEL_CAP;
        assert_eq!(SkillRow::GrenadeDamage.cost(&levels, &unlocks), None);

        // Unlocks cost their price once, then never again.
        assert_eq!(
            SkillRow::Nova.cost(&levels, &unlocks),
            Some(NOVA_UNLOCK_COST)
        );
        unlocks.nova = true;
        assert_eq!(SkillRow::Nova.cost(&levels, &unlocks), None);
        assert_eq!(
            SkillRow::DeflectVolley.cost(&levels, &unlocks),
            Some(VOLLEY_UNLOCK_COST)
        );
        unlocks.deflect_volley = true;
        assert_eq!(SkillRow::DeflectVolley.cost(&levels, &unlocks), None);
    }

    #[test]
    fn rows_grey_out_on_cost_and_ownership() {
        let levels = SkillLevels::default();
        let mut unlocks = AbilityUnlocks::default();
        assert!(!SkillRow::Cleave.available(0, &levels, &unlocks));
        assert!(SkillRow::Cleave.available(1, &levels, &unlocks));
        assert!(!SkillRow::Dash.available(1, &levels, &unlocks));
        assert!(SkillRow::Dash.available(2, &levels, &unlocks));
        unlocks.dash = true;
        assert!(!SkillRow::Dash.available(99, &levels, &unlocks));
        assert!(SkillRow::Grenade.available(3, &levels, &unlocks));
        assert!(SkillRow::Vitality.available(1, &levels, &unlocks));
        assert!(!SkillRow::Barrier.available(2, &levels, &unlocks));
        assert!(SkillRow::Barrier.available(3, &levels, &unlocks));
        unlocks.barrier = true;
        assert!(!SkillRow::Barrier.available(99, &levels, &unlocks));
        // Capped rows grey out even with points to burn.
        let full = SkillLevels {
            shield_duration: SHIELD_DURATION_CAP,
            ..Default::default()
        };
        assert!(!SkillRow::ShieldDuration.available(99, &full, &unlocks));
        // The new unlocks price in.
        assert!(!SkillRow::Nova.available(3, &levels, &unlocks));
        assert!(SkillRow::Nova.available(4, &levels, &unlocks));
        assert!(!SkillRow::DeflectVolley.available(4, &levels, &unlocks));
        assert!(SkillRow::DeflectVolley.available(5, &levels, &unlocks));
    }

    #[test]
    fn labels_show_level_and_cost() {
        let mut levels = SkillLevels::default();
        let unlocks = AbilityUnlocks::default();
        // A fresh capped row shows the cap and cost.
        let label = SkillRow::CleaveReach.label(&levels, &unlocks);
        assert!(
            label.contains("Lv 0/4") && label.contains("cost 1"),
            "{label}"
        );
        // A maxed row says so instead of a cost.
        levels.cleave_reach = CLEAVE_REACH_CAP;
        let label = SkillRow::CleaveReach.label(&levels, &unlocks);
        assert!(
            label.contains("Lv 4/4") && label.contains("maxed"),
            "{label}"
        );
        // A gated row says what it needs.
        let label = SkillRow::DashCooldown.label(&levels, &unlocks);
        assert!(label.contains("needs dash"), "{label}");
        // An owned unlock stops advertising itself.
        let unlocks = AbilityUnlocks {
            nova: true,
            ..Default::default()
        };
        let label = SkillRow::Nova.label(&levels, &unlocks);
        assert!(label.contains("nova unlocked"), "{label}");
    }
}
