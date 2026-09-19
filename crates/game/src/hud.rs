//! Heads-up display: HP bar and the off-screen exit-direction arrow. Both are
//! bevy_ui nodes over the world; the pure layout maths lives in small tested
//! helpers.

use avian2d::prelude::Position;
use bevy::math::Rot2;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::combat::{MAX_HP, PlayerVitals};
use crate::world::WorldMapRes;

/// Screen-border margin for the exit arrow, in pixels.
const ARROW_MARGIN: f32 = 24.0;

/// On-screen size of the exit arrow, in pixels.
const ARROW_SIZE: f32 = 28.0;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, (update_hp_bar, update_exit_arrow));
    }
}

/// Marker for the HP bar's fill node.
#[derive(Component)]
struct HpFill;

/// Marker for the off-screen exit-direction arrow.
#[derive(Component)]
struct ExitArrow;

fn spawn_hud(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(16.0),
                bottom: Val::Px(16.0),
                width: Val::Px(220.0),
                height: Val::Px(14.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_children(|bar| {
            bar.spawn((
                HpFill,
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(hp_fill_colour(1.0)),
            ));
        });

    // Off-screen exit-direction arrow, hidden until the exit leaves the
    // viewport (the beacon pillar covers the on-screen case).
    let arrow: Handle<Image> = images.add(arrow_image());
    commands.spawn((
        ExitArrow,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(ARROW_SIZE),
            height: Val::Px(ARROW_SIZE),
            ..default()
        },
        UiTransform::IDENTITY,
        ImageNode::new(arrow),
        Visibility::Hidden,
    ));
}

/// Alpha of one pixel of the procedural arrow image (a triangle pointing
/// up — the screen's negative-y direction), 0 for background.
fn arrow_alpha(col: u32, row: u32, size: u32) -> u8 {
    let apex = 4u32;
    let base = size.saturating_sub(4);
    if row < apex || row >= base {
        return 0;
    }
    // Widen from the apex down to a 24-px base.
    let half_width = 1 + ((row - apex) * 11) / (base - apex);
    let centre = size / 2;
    if col.abs_diff(centre) <= half_width {
        255
    } else {
        0
    }
}

/// Build the arrow image: a warm gold triangle pointing up (negative y).
fn arrow_image() -> Image {
    const S: u32 = 32;
    let mut data = vec![0u8; (S * S * 4) as usize];
    for row in 0..S {
        for col in 0..S {
            let alpha = arrow_alpha(col, row, S);
            if alpha > 0 {
                let i = ((row * S + col) * 4) as usize;
                data[i] = 255;
                data[i + 1] = 220;
                data[i + 2] = 80;
                data[i + 3] = alpha;
            }
        }
    }
    Image::new(
        Extent3d {
            width: S,
            height: S,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::MAIN_WORLD | bevy::asset::RenderAssetUsages::RENDER_WORLD,
    )
}

/// Bind the fill's width and colour to the player's current HP.
fn update_hp_bar(
    vitals: Res<PlayerVitals>,
    mut fill: Single<(&mut Node, &mut BackgroundColor), With<HpFill>>,
) {
    let fraction = hp_fill_fraction(vitals.hp, MAX_HP);
    fill.0.width = Val::Percent(fraction * 100.0);
    fill.1.0 = hp_fill_colour(fraction);
}

/// Pin a viewport position to inside the window border.
fn clamp_to_border(pos: Vec2, bounds: Vec2, margin: f32) -> Vec2 {
    Vec2::new(
        pos.x.clamp(margin, (bounds.x - margin).max(margin)),
        pos.y.clamp(margin, (bounds.y - margin).max(margin)),
    )
}

/// Rotation (clockwise, per `UiTransform`) turning an up-pointing arrow to
/// point along `screen_dir` in y-down viewport space.
fn arrow_rotation(screen_dir: Vec2) -> f32 {
    screen_dir.y.atan2(screen_dir.x) + core::f32::consts::FRAC_PI_2
}

/// Point the arrow at the exit whenever the exit is off-screen.
fn update_exit_arrow(
    map: Res<WorldMapRes>,
    config: Res<crate::world::MapConfig>,
    player: Single<&Position, With<crate::Player>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    arrow: Single<(&mut Node, &mut UiTransform, &mut Visibility), With<ExitArrow>>,
) {
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let exit_world = crate::world::tile_world_pos((width, height), map.map.exit, config.tile_size);

    let (Some(bounds), Ok(exit_vp), Ok(player_vp)) = (
        camera.0.logical_viewport_size(),
        camera.0.world_to_viewport(camera.1, exit_world.extend(0.0)),
        camera.0.world_to_viewport(camera.1, player.0.extend(0.0)),
    ) else {
        arrow.into_inner().2.set_if_neq(Visibility::Hidden);
        return;
    };

    let (mut node, mut transform, mut visibility) = arrow.into_inner();
    let on_screen =
        exit_vp.x >= 0.0 && exit_vp.y >= 0.0 && exit_vp.x <= bounds.x && exit_vp.y <= bounds.y;
    if on_screen {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    }
    let clamped = clamp_to_border(exit_vp, bounds, ARROW_MARGIN);
    node.left = Val::Px(clamped.x - ARROW_SIZE / 2.0);
    node.top = Val::Px(clamped.y - ARROW_SIZE / 2.0);
    // Aim from the player's viewport position, NOT the clamped one — the
    // clamped point would flatten the angle along the border.
    transform.rotation = Rot2::radians(arrow_rotation(exit_vp - player_vp));
    visibility.set_if_neq(Visibility::Visible);
}

/// Clamped HP fraction (0..=1) for the bar fill.
fn hp_fill_fraction(hp: i32, max_hp: i32) -> f32 {
    if max_hp <= 0 {
        return 0.0;
    }
    hp.clamp(0, max_hp) as f32 / max_hp as f32
}

/// Fill colour: healthy is white, draining shifts toward red.
fn hp_fill_colour(fraction: f32) -> Color {
    Color::srgb(1.0, 0.25 + 0.75 * fraction, 0.25 + 0.75 * fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hp_fraction_clamps_and_handles_bad_max() {
        assert_eq!(hp_fill_fraction(100, 100), 1.0);
        assert_eq!(hp_fill_fraction(50, 100), 0.5);
        assert_eq!(hp_fill_fraction(0, 100), 0.0);
        assert_eq!(hp_fill_fraction(-10, 100), 0.0);
        assert_eq!(hp_fill_fraction(150, 100), 1.0);
        assert_eq!(hp_fill_fraction(50, 0), 0.0);
    }

    #[test]
    fn hp_colour_drains_from_white_toward_red() {
        let healthy = hp_fill_colour(1.0);
        let drained = hp_fill_colour(0.0);
        // Healthy: full green/blue; drained: red-dominant.
        assert!(healthy.to_srgba().green > drained.to_srgba().green);
        assert!((healthy.to_srgba().red - 1.0).abs() < 1e-6);
        assert!((drained.to_srgba().green - 0.25).abs() < 1e-6);
    }

    #[test]
    fn border_clamp_keeps_the_arrow_inside() {
        let bounds = Vec2::new(800.0, 600.0);
        assert_eq!(
            clamp_to_border(Vec2::new(-50.0, 300.0), bounds, 24.0).x,
            24.0
        );
        assert_eq!(
            clamp_to_border(Vec2::new(900.0, -50.0), bounds, 24.0),
            Vec2::new(776.0, 24.0)
        );
        // Inside positions pass through untouched.
        assert_eq!(
            clamp_to_border(Vec2::new(400.0, 300.0), bounds, 24.0),
            Vec2::new(400.0, 300.0)
        );
        // Tiny windows never produce an inverted clamp range.
        assert_eq!(
            clamp_to_border(Vec2::new(5.0, 5.0), Vec2::new(20.0, 20.0), 24.0),
            Vec2::new(24.0, 24.0)
        );
    }

    #[test]
    fn arrow_rotation_matches_screen_directions() {
        use core::f32::consts::{FRAC_PI_2, PI, TAU};
        let rotation = |dir: Vec2| arrow_rotation(dir).rem_euclid(TAU);
        // Y is DOWN in viewport space; the arrow texture points up, and
        // UiTransform rotates clockwise on screen.
        assert!(rotation(Vec2::new(0.0, -1.0)).abs() < 1e-6); // up: no turn
        assert!((rotation(Vec2::new(1.0, 0.0)) - FRAC_PI_2).abs() < 1e-6); // right
        assert!((rotation(Vec2::new(0.0, 1.0)) - PI).abs() < 1e-6); // down
        assert!((rotation(Vec2::new(-1.0, 0.0)) - 3.0 * FRAC_PI_2).abs() < 1e-6); // left
    }

    #[test]
    fn arrow_texture_widens_toward_the_base() {
        let s = 32;
        assert!(arrow_alpha(16, 4, s) > 0, "apex is drawn");
        assert_eq!(arrow_alpha(8, 4, s), 0, "apex row is narrow");
        assert!(arrow_alpha(8, 27, s) > 0, "base is wide");
        assert_eq!(arrow_alpha(2, 27, s), 0, "base stops short of the frame");
        assert_eq!(arrow_alpha(16, 0, s), 0, "nothing above the apex");
    }
}
