//! The voicing contour: paired Bézier curves bounding the instruments'
//! register, ported from submarine's arrangement curves.
//!
//! A curve is two control-point lists — upper and lower voicing bounds, in
//! semitones from the tonic. `contour(t)` samples the bounds at the phrase's
//! parameter; `extend_from` starts a curve at the previous curve's tail
//! control point so consecutive curves stitch without a seam. Patterns bend
//! into the bounds ("the arp exists below the curve").

/// The default contour: bounds generous enough that today's material never
/// clips — musically neutral until a composer curve replaces it.
pub fn default_curve() -> Curve {
    Curve {
        upper: vec![48.0, 50.0, 49.0, 48.0],
        lower: vec![10.0, 12.0, 12.0, 10.0],
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
    fn the_default_curve_never_clips_today_s_material() {
        // Pattern B tops out 42 semitones above the tonic (the C# slot's
        // widest voicing, two octaves up); pattern A's floor sits 12 up.
        // The bounds clear both across the whole arc — a four-point Bézier
        // pulls inside its control points, so this checks the arc, not the
        // endpoints.
        let curve = default_curve();
        for step in 0..=16 {
            let (low, high) = curve.contour(step as f64 / 16.0);
            assert!(high >= 42.0, "upper bound clips pattern B at step {step}");
            assert!(low <= 12.0, "lower bound clips pattern A at step {step}");
        }
    }
}
