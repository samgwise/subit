//! The voicing contour: paired Bézier curves bounding the instruments'
//! register, ported from submarine's arrangement curves.
//!
//! A curve is two control-point lists — upper and lower voicing bounds, in
//! semitones from the tonic. `contour(t)` samples the bounds at the phrase's
//! parameter; `extend_from` starts a curve at the previous curve's tail
//! control point so consecutive curves stitch without a seam. Patterns bend
//! into the bounds ("the arp exists below the curve"), and the sustained
//! voices' register bands derive from the same bounds — the curve IS the
//! arrangement space.
//!
//! The deterministic cycle tiles between a library of named shapes (rolled
//! per turn, stitched from the previous tail), and the conductor's intensity
//! scales the span of whatever curve is live — combat blooms it out, calm
//! folds it home.

use rand::RngExt;
use rand::rngs::SmallRng;

/// The default contour: the wide neutral arc — the cycle's initial fill
/// until the first tile is rolled.
pub fn default_curve() -> Curve {
    ContourShape::Arc.curve()
}

/// The register centre the span folds bounds toward, in semitones from
/// the tonic — the middle of the instruments' range.
pub const SPAN_CENTRE: f64 = 28.0;

/// The span factor at rest — how far the curve pulls in when the score
/// sits still (1.0 is the drawn shape's full spread).
pub const SPAN_REST: f32 = 0.55;

/// The intensity-mapped span factor: `SPAN_REST` at rest rising to 1.0 at
/// full flight. Monotone in the (clamped) intensity.
pub fn span_factor(intensity: f32) -> f32 {
    SPAN_REST + (1.0 - SPAN_REST) * intensity.clamp(0.0, 1.0)
}

/// The curve's bounds pulled toward [`SPAN_CENTRE`] by the intensity's
/// span factor. The dynamic arc reads shapes normalised to their own
/// hull — which the fold preserves — so the span is for the register
/// contour alone.
pub fn effective_curve(curve: &Curve, intensity: f32) -> Curve {
    let k = f64::from(span_factor(intensity));
    let fold = |points: &[f64]| {
        points
            .iter()
            .map(|&point| SPAN_CENTRE + (point - SPAN_CENTRE) * k)
            .collect()
    };
    Curve {
        upper: fold(&curve.upper),
        lower: fold(&curve.lower),
    }
}

/// The named voicing contours the cycle tiles between — each drawn at
/// full spread within the instruments' ranges (semitones from the tonic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContourShape {
    /// The wide neutral arc — the original default.
    Arc,
    /// An ascending sweep: the phrase climbs the register.
    Rise,
    /// A descending sweep: the phrase sinks home.
    Fall,
    /// Open high, dipping low through the middle.
    Valley,
    /// A tight band around the middle register.
    Narrow,
}

impl ContourShape {
    /// Roll the next tile — every shape equally likely.
    pub fn roll(rng: &mut SmallRng) -> Self {
        const ALL: [ContourShape; 5] = [
            ContourShape::Arc,
            ContourShape::Rise,
            ContourShape::Fall,
            ContourShape::Valley,
            ContourShape::Narrow,
        ];
        ALL[rng.random_range(0..ALL.len())]
    }

    /// The shape's curve, drawn at full spread.
    pub fn curve(self) -> Curve {
        let (upper, lower) = match self {
            ContourShape::Arc => (vec![48.0, 50.0, 49.0, 48.0], vec![10.0, 12.0, 12.0, 10.0]),
            ContourShape::Rise => (vec![30.0, 36.0, 42.0, 48.0], vec![8.0, 10.0, 12.0, 14.0]),
            ContourShape::Fall => (vec![48.0, 42.0, 36.0, 30.0], vec![14.0, 12.0, 10.0, 8.0]),
            ContourShape::Valley => (vec![44.0, 30.0, 44.0], vec![16.0, 8.0, 16.0]),
            ContourShape::Narrow => (vec![40.0, 42.0, 40.0], vec![18.0, 22.0, 18.0]),
        };
        Curve { upper, lower }
    }
}

/// The named dynamic arcs the cycle tiles between — shapes read 0–1
/// relative to their own hull (see `music::curve_norm`), so only the
/// silhouette matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicShape {
    /// The gentle swell and settle — the original default.
    Swell,
    /// A mid-phrase dip.
    Dip,
    /// A steady climb to the phrase's end.
    Climb,
    /// A die-away from the opening.
    Fade,
}

impl DynamicShape {
    /// Roll the next tile — every shape equally likely.
    pub fn roll(rng: &mut SmallRng) -> Self {
        const ALL: [DynamicShape; 4] = [
            DynamicShape::Swell,
            DynamicShape::Dip,
            DynamicShape::Climb,
            DynamicShape::Fade,
        ];
        ALL[rng.random_range(0..ALL.len())]
    }

    /// The shape's arc — the same silhouette on both sides of the hull.
    pub fn curve(self) -> Curve {
        let points = match self {
            DynamicShape::Swell => vec![0.0, 1.0, 0.6, 0.0],
            DynamicShape::Dip => vec![0.9, 0.1, 0.9],
            DynamicShape::Climb => vec![0.0, 0.4, 0.7, 1.0],
            DynamicShape::Fade => vec![1.0, 0.6, 0.2, 0.0],
        };
        Curve {
            upper: points.clone(),
            lower: points,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub upper: Vec<f64>,
    pub lower: Vec<f64>,
}

impl Curve {
    /// The voicing bounds at phrase parameter `t` (0–1), sorted.
    pub fn contour(&self, t: f64) -> (f64, f64) {
        let mut bounds = [bezier(t, &self.upper), bezier(t, &self.lower)];
        bounds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        (bounds[0], bounds[1])
    }

    /// A curve starting from this one's tail control point, so consecutive
    /// curves join without a seam.
    pub fn extend_from(&self, tail: &Curve) -> Curve {
        let stitch = |points: &[f64], previous: &[f64]| {
            let mut stitched = Vec::with_capacity(previous.len() + points.len());
            stitched.push(previous.last().copied().unwrap_or(0.0));
            stitched.extend_from_slice(points);
            stitched
        };
        Curve {
            upper: stitch(&self.upper, &tail.upper),
            lower: stitch(&self.lower, &tail.lower),
        }
    }
}

/// A generalised Bézier over `points` control values (de Casteljau) — works
/// for any point count; two points linearly interpolate.
fn bezier(t: f64, points: &[f64]) -> f64 {
    let mut work = points.to_vec();
    while work.len() > 1 {
        work = work
            .windows(2)
            .map(|pair| pair[0] + (pair[1] - pair[0]) * t)
            .collect();
    }
    work.first().copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contour_samples_the_bounds_in_order() {
        let curve = Curve { upper: vec![30.0, 34.0], lower: vec![8.0, 12.0] };
        let (low, high) = curve.contour(0.0);
        assert!((low - 8.0).abs() < 1e-9 && (high - 30.0).abs() < 1e-9);
        let (low, high) = curve.contour(1.0);
        assert!((low - 12.0).abs() < 1e-9 && (high - 34.0).abs() < 1e-9);
        // Halfway is halfway — Béziers of two points are lines.
        let (low, _) = curve.contour(0.5);
        assert!((low - 10.0).abs() < 1e-9);
    }

    #[test]
    fn control_points_bend_the_contour() {
        let curve = Curve { upper: vec![20.0, 20.0, 40.0], lower: vec![10.0, 10.0, 10.0] };
        // The middle control point pulls the curve up but not all the way.
        let (low, high) = curve.contour(0.5);
        assert!((low - 10.0).abs() < 1e-9);
        assert!(high > 20.0 && high < 40.0, "the bend is partial: {high}");
    }

    #[test]
    fn curves_stitch_from_the_previous_tail() {
        let first = Curve { upper: vec![30.0, 32.0, 40.0], lower: vec![10.0, 10.0, 12.0] };
        let second = Curve { upper: vec![36.0, 30.0], lower: vec![14.0, 12.0] };
        let stitched = second.extend_from(&first);
        assert_eq!(stitched.upper, vec![40.0, 36.0, 30.0]);
        assert_eq!(stitched.lower, vec![12.0, 14.0, 12.0]);
        // The seam is exact: contour(0) of the stitched curve is the tail.
        let (low, high) = stitched.contour(0.0);
        assert!((low - 12.0).abs() < 1e-9 && (high - 40.0).abs() < 1e-9);
    }

    #[test]
    fn every_shape_stays_inside_the_instruments_range() {
        for shape in [
            ContourShape::Arc,
            ContourShape::Rise,
            ContourShape::Fall,
            ContourShape::Valley,
            ContourShape::Narrow,
        ] {
            let curve = shape.curve();
            for step in 0..=16 {
                let (low, high) = curve.contour(step as f64 / 16.0);
                assert!(low <= high, "{shape:?} bounds cross at step {step}");
                assert!(low >= 8.0, "{shape:?} clips the pad floor at step {step}");
                assert!(high <= 50.0, "{shape:?} clips the ceiling at step {step}");
            }
        }
    }

    #[test]
    fn a_rolled_tile_stitches_from_the_tail() {
        // Whatever the roll picks, the tile starts where the last curve
        // ended — the same seam test the composer's curves answer to.
        let tail = Curve { upper: vec![30.0, 32.0, 40.0], lower: vec![10.0, 10.0, 12.0] };
        for shape in [
            ContourShape::Arc,
            ContourShape::Rise,
            ContourShape::Fall,
            ContourShape::Valley,
            ContourShape::Narrow,
        ] {
            let stitched = shape.curve().extend_from(&tail);
            let (low, high) = stitched.contour(0.0);
            let (tail_low, tail_high) = tail.contour(1.0);
            assert!((low - tail_low).abs() < 1e-9, "{shape:?} breaks the lower seam");
            assert!((high - tail_high).abs() < 1e-9, "{shape:?} breaks the upper seam");
        }
    }

    #[test]
    fn the_span_folds_at_rest_and_releases_in_full_flight() {
        let curve = ContourShape::Arc.curve();
        // Full flight is the identity.
        let spread = effective_curve(&curve, 1.0);
        assert_eq!(spread, curve);
        // At rest the bounds pull in toward the centre — the floor rises,
        // the ceiling falls, and the bounds stay ordered.
        let folded = effective_curve(&curve, 0.0);
        for step in 0..=8 {
            let t = step as f64 / 8.0;
            let (rest_low, rest_high) = folded.contour(t);
            let (full_low, full_high) = curve.contour(t);
            assert!(rest_low > full_low, "the floor did not rise at step {step}");
            assert!(rest_high < full_high, "the ceiling did not fall at step {step}");
        }
        // The factor is monotone in the intensity.
        let mut last = 0.0;
        for step in 0..=10 {
            let factor = span_factor(step as f32 / 10.0);
            assert!(factor >= last);
            last = factor;
        }
        assert!((last - 1.0).abs() < 1e-6);
    }
}
