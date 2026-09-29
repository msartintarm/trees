//! The sky's clockwork: where the sun (or moon) stands for a time of day
//! and season, what color its light is, how bright the day is — and how
//! the day blurs when time runs fast. A long-lived observer watching years
//! pass doesn't see days flicker; the sun smears into its daily arc (the
//! long-exposure "sun trail"), and the light settles to the day's average.
//! That blur is also what keeps fast time from strobing the screen.
//!
//! Map axes: +x east, +y north, +z up.

/// Latitude of the map (degrees north): mid-latitude, real seasons.
pub const LATITUDE_DEG: f64 = 40.0;
/// Earth's axial tilt (the seasonal swing of the sun's declination).
pub const TILT_DEG: f64 = 23.44;
/// Hour angle of the representative sun a blurred day settles to: late
/// morning in the southeast, close to the legacy fixed sun.
const REP_HOUR: f64 = -0.45;

/// How the scene is lit: the clock's real time of day, or a fixed look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LightMode {
    /// Follow the time of day (blurred when time runs fast).
    #[default]
    Clock,
    GoldenHour,
    Dusk,
    Night,
}

impl LightMode {
    pub fn from_u8(v: u8) -> LightMode {
        match v {
            1 => LightMode::GoldenHour,
            2 => LightMode::Dusk,
            3 => LightMode::Night,
            _ => LightMode::Clock,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunState {
    /// Direction toward the key light (the sun, or the moon at night),
    /// normalized; always at least a little above the horizon so shadows
    /// stay stable.
    pub light_dir: [f64; 3],
    /// The sun's true direction (may be below the horizon).
    pub sun_dir: [f64; 3],
    /// Key-light color × intensity (the shader's sun term).
    pub light_color: [f64; 3],
    /// Daylight, 0 (night) .. 1 (full day): sky brightness and ambient.
    pub daylight: f64,
    /// 0 = a crisp sun disc; 1 = fully smeared into its daily arc.
    pub trail: f64,
    /// Solar declination (radians) — the arc the trail follows.
    pub declination: f64,
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn lerp3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Solar declination for a point in the year (0 = spring equinox).
pub fn declination(year_frac: f64) -> f64 {
    TILT_DEG.to_radians() * (std::f64::consts::TAU * year_frac).sin()
}

/// Sun direction for an hour angle (0 = solar noon, radians) and
/// declination at the map's latitude.
pub fn sun_direction(hour_angle: f64, decl: f64) -> [f64; 3] {
    let phi = LATITUDE_DEG.to_radians();
    let (sd, cd) = decl.sin_cos();
    let (sp, cp) = phi.sin_cos();
    let (sh, ch) = hour_angle.sin_cos();
    [-cd * sh, sd * cp - cd * sp * ch, sd * sp + cd * cp * ch]
}

/// The afternoon hour angle at which the sun stands at `alt_sin` for a
/// declination.
fn afternoon_hour(alt_sin: f64, decl: f64) -> f64 {
    let phi = LATITUDE_DEG.to_radians();
    let c = (alt_sin - decl.sin() * phi.sin()) / (decl.cos() * phi.cos());
    c.clamp(-1.0, 1.0).acos()
}

/// Days per real second above which days begin to blur, and where the
/// blur is complete.
pub const BLUR_START: f64 = 0.25;
pub const BLUR_FULL: f64 = 2.0;

/// How smeared the day is at a rate of time (game days per real second).
pub fn blur_for_rate(days_per_sec: f64) -> f64 {
    smoothstep(BLUR_START.ln(), BLUR_FULL.ln(), days_per_sec.max(1e-9).ln())
}

/// Sunlight color by solar altitude: white-gold at height, amber at golden
/// hour, deep red at the horizon.
fn sun_color(alt_sin: f64) -> [f64; 3] {
    let high = [1.0, 0.98, 0.94];
    let gold = [1.0, 0.72, 0.42];
    let red = [0.95, 0.42, 0.25];
    if alt_sin > 0.3 {
        lerp3(gold, high, smoothstep(0.3, 0.6, alt_sin))
    } else {
        lerp3(red, gold, smoothstep(0.0, 0.3, alt_sin))
    }
}

/// The key light for a day fraction (0 = midnight, 0.5 = noon), year
/// fraction (0 = spring), blur (0 crisp .. 1 averaged), and light mode.
pub fn sun_state(day_frac: f64, year_frac: f64, blur: f64, mode: LightMode) -> SunState {
    let decl = declination(year_frac);
    let (hour, blur) = match mode {
        LightMode::Clock => (std::f64::consts::TAU * (day_frac - 0.5), blur.clamp(0.0, 1.0)),
        // Late afternoon, in the west: the hour at which this season's sun
        // stands at the look's altitude.
        LightMode::GoldenHour => (afternoon_hour(0.14, decl), 0.0),
        LightMode::Dusk => (afternoon_hour(-0.03, decl), 0.0),
        LightMode::Night => (std::f64::consts::PI, 0.0),
    };
    let actual = sun_direction(hour, decl);
    let rep = sun_direction(REP_HOUR, decl);
    // Blend the sun toward its representative daytime position (along the
    // great circle, via normalized lerp — the blur is a look, not optics).
    let sun = normalize(lerp3(actual, rep, blur));
    let alt = sun[2];
    let daylight = smoothstep(-0.12, 0.10, alt);
    // Moonlight: a dim blue key from high in the opposite sky.
    let moon = normalize([-sun[0] * 0.6, -sun[1] * 0.6 + 0.2, 0.75]);
    let sun_key = sun_color(alt.max(0.0));
    let key_sun = smoothstep(-0.02, 0.12, alt);
    let light_color = lerp3([0.34, 0.42, 0.62], sun_key, key_sun);
    let intensity = 0.32 + 0.68 * key_sun;
    let light_dir = if key_sun > 0.0 && alt > 0.02 {
        normalize([sun[0], sun[1], sun[2].max(0.12)])
    } else if alt > -0.02 {
        // Sunset/sunrise: keep the sun's azimuth, just above the horizon.
        normalize([sun[0], sun[1], 0.12])
    } else {
        moon
    };
    SunState {
        light_dir,
        sun_dir: sun,
        light_color: light_color.map(|c| c * intensity),
        daylight,
        trail: if mode == LightMode::Clock { blur } else { 0.0 },
        declination: decl,
    }
}

/// The legacy look: the fixed overview sun of earlier versions.
pub fn legacy() -> SunState {
    let d = crate::sim::terrain::SUN_DIR;
    SunState {
        light_dir: d,
        sun_dir: d,
        light_color: [1.0, 1.0, 1.0],
        daylight: 1.0,
        trail: 1.0,
        declination: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_higher_in_summer_than_winter() {
        let summer = sun_direction(0.0, declination(0.25));
        let winter = sun_direction(0.0, declination(0.75));
        assert!(summer[2] > winter[2] + 0.5);
        // Noon sun stands due south at mid-northern latitudes.
        assert!(summer[1] < 0.0 && summer[0].abs() < 1e-9);
    }

    #[test]
    fn the_sun_rises_in_the_east_and_sets_in_the_west() {
        let morning = sun_direction(-1.2, 0.0);
        let evening = sun_direction(1.2, 0.0);
        assert!(morning[0] > 0.3 && evening[0] < -0.3);
    }

    #[test]
    fn nights_are_dark_and_days_bright() {
        let night = sun_state(0.0, 0.25, 0.0, LightMode::Clock);
        let noon = sun_state(0.5, 0.25, 0.0, LightMode::Clock);
        assert!(night.daylight < 0.05 && noon.daylight > 0.95);
        assert!(night.light_dir[2] > 0.3, "moonlight from above");
        let lum = |s: &SunState| s.light_color.iter().sum::<f64>();
        assert!(lum(&noon) > 2.0 * lum(&night));
    }

    #[test]
    fn fast_time_blurs_the_day_so_nothing_flickers() {
        assert_eq!(blur_for_rate(0.05), 0.0);
        assert_eq!(blur_for_rate(10.0), 1.0);
        // Fully blurred, midnight and noon light the scene identically.
        let a = sun_state(0.0, 0.1, 1.0, LightMode::Clock);
        let b = sun_state(0.5, 0.1, 1.0, LightMode::Clock);
        for k in 0..3 {
            assert!((a.light_dir[k] - b.light_dir[k]).abs() < 1e-9);
            assert!((a.light_color[k] - b.light_color[k]).abs() < 1e-9);
        }
        assert!(a.daylight > 0.95);
    }

    #[test]
    fn golden_hour_is_low_and_warm() {
        let g = sun_state(0.0, 0.0, 1.0, LightMode::GoldenHour);
        assert!(g.sun_dir[2] > 0.0 && g.sun_dir[2] < 0.3, "alt {}", g.sun_dir[2]);
        assert!(g.light_color[0] > g.light_color[2] * 1.5, "warm light");
        assert!(g.sun_dir[0] < 0.0, "in the west");
    }
}
