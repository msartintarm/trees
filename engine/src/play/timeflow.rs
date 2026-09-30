//! Time as the wanderer experiences it. The player is long-lived: time is
//! measured in the ecology's years, and how fast it passes depends on what
//! the player does:
//!
//! - **Standing still**, time nearly stops — about an hour of game time per
//!   real second, so a whole day and night can be watched go by.
//! - **Moving**, time flows in proportion to speed: walking passes a year in
//!   about half a minute, sprinting a year in ~12 s. Distance travelled is
//!   time spent, so the forest you walked away from has aged by the time you
//!   come back — in proportion to how far you went.
//! - **Actions** (felling, building, planting) are *time-lapses*: each
//!   costs a span of game time that plays out quickly over a second or
//!   three, sweeping the seasons forward.
//! - **Resting** (held) lets years stream by.
//!
//! Pure data; the bridge feeds it real seconds and the player's speed, and
//! runs one ecology tick per whole year crossed.

/// Years of game time per real second standing still (1 game hour / s).
pub const IDLE_PACE: f64 = 1.0 / 8760.0;
/// Extra years per real second at walking speed (a year per 30 s walk).
pub const WALK_PACE: f64 = 1.0 / 30.0;
/// Years per real second while resting.
pub const REST_PACE: f64 = 1.5;
/// Time constant for pace changes (s): time eases in and out rather than
/// lurching when the player starts or stops.
const PACE_EASE: f64 = 0.45;

/// An action's cost in game time and how long its time-lapse plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionCost {
    pub years: f64,
    pub secs: f64,
}

/// Planting a sapling or a sod plug: a few weeks' tending.
pub const PLANT: ActionCost = ActionCost { years: 0.06, secs: 0.8 };
/// Felling and bucking a tree: a season's hard work.
pub const FELL: ActionCost = ActionCost { years: 0.25, secs: 1.6 };
/// Raising a house: two years of labor.
pub const BUILD: ActionCost = ActionCost { years: 2.0, secs: 3.2 };
/// Lighting a fire: no time at all — the burn itself takes years.
pub const IGNITE: ActionCost = ActionCost { years: 0.01, secs: 0.3 };

#[derive(Clone, Debug)]
pub struct TimeFlow {
    /// Game time since the walk began, in years (fraction = season).
    years: f64,
    /// Smoothed continuous pace, years per real second.
    pace: f64,
    /// An action's time-lapse in progress: years left, and its rate.
    lapse_left: f64,
    lapse_rate: f64,
    lapse_label: &'static str,
    /// The last frame's total rate (years / s), for the day blur.
    rate: f64,
}

impl TimeFlow {
    /// Start at a point in the year (0 = spring) and time of day
    /// (0 = midnight, 0.5 = noon).
    pub fn new(year_frac: f64, day_frac: f64) -> TimeFlow {
        let year = year_frac.rem_euclid(1.0);
        // Put the day fraction at `day_frac` within the chosen year point.
        let days = (year * 365.0).floor() + day_frac.rem_euclid(1.0);
        TimeFlow {
            years: days / 365.0,
            pace: IDLE_PACE,
            lapse_left: 0.0,
            lapse_rate: 0.0,
            lapse_label: "",
            rate: IDLE_PACE,
        }
    }

    pub fn years(&self) -> f64 {
        self.years
    }

    /// Position in the year, 0..1 (0 = start of spring).
    pub fn year_frac(&self) -> f64 {
        self.years.rem_euclid(1.0)
    }

    /// Time of day, 0..1 (0.5 = noon).
    pub fn day_frac(&self) -> f64 {
        (self.years * 365.0).rem_euclid(1.0)
    }

    /// Current rate of time, game days per real second.
    pub fn days_per_sec(&self) -> f64 {
        self.rate * 365.0
    }

    /// The action being time-lapsed, if any, and the years still to pass.
    pub fn lapse(&self) -> Option<(&'static str, f64)> {
        (self.lapse_left > 0.0).then_some((self.lapse_label, self.lapse_left))
    }

    /// Whether an action is still playing out (no new action starts then).
    pub fn busy(&self) -> bool {
        self.lapse_left > 0.0
    }

    /// Begin an action's time-lapse.
    pub fn act(&mut self, cost: ActionCost, label: &'static str) {
        self.lapse_left += cost.years;
        self.lapse_rate = self.lapse_left / cost.secs.max(0.05);
        self.lapse_label = label;
    }

    /// Advance by `dt` real seconds. `motion` is the player's speed as a
    /// multiple of walking speed (0 standing, 1 walking, ~2.6 sprinting);
    /// `resting` holds time open. Returns the game years that passed.
    pub fn step(&mut self, dt: f64, motion: f64, resting: bool) -> f64 {
        if dt <= 0.0 {
            return 0.0;
        }
        let target = if resting { REST_PACE } else { IDLE_PACE + WALK_PACE * motion.max(0.0) };
        // Ease in log space: from an hour per second to years per second
        // is a factor of 10⁴, and a linear ease would jump the low end.
        let k = 1.0 - (-dt / PACE_EASE).exp();
        self.pace = (self.pace.ln() + (target.ln() - self.pace.ln()) * k).exp();
        let mut advanced = self.pace * dt;
        if self.lapse_left > 0.0 {
            let lapse = (self.lapse_rate * dt).min(self.lapse_left);
            self.lapse_left -= lapse;
            advanced += lapse;
            if self.lapse_left <= 1e-12 {
                self.lapse_left = 0.0;
            }
        }
        self.years += advanced;
        self.rate = advanced / dt;
        advanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut TimeFlow, secs: f64, motion: f64) -> f64 {
        let mut total = 0.0;
        let steps = (secs * 60.0) as usize;
        for _ in 0..steps {
            total += t.step(1.0 / 60.0, motion, false);
        }
        total
    }

    #[test]
    fn standing_still_time_nearly_stops_and_walking_passes_seasons() {
        let mut t = TimeFlow::new(0.0, 0.5);
        run(&mut t, 3.0, 0.0);
        let still = run(&mut t, 10.0, 0.0);
        assert!(still < 0.002, "10 s standing = {:.4} years", still);
        assert!(still * 365.0 > 0.3, "…but a day's light still moves ({:.2} days)", still * 365.0);
        run(&mut t, 3.0, 1.0);
        let walked = run(&mut t, 30.0, 1.0);
        assert!((0.8..1.2).contains(&walked), "30 s walking ≈ a year: {walked:.2}");
        run(&mut t, 3.0, 2.6);
        let sprint = run(&mut t, 12.0, 2.6);
        assert!(sprint > walked * 0.9, "sprinting 12 s ≈ walking 30 s: {sprint:.2}");
    }

    #[test]
    fn time_passes_in_proportion_to_distance() {
        // Walk 30 s at speed 1 vs 15 s at speed 2: the same distance,
        // nearly the same time (only the idle trickle differs).
        let mut a = TimeFlow::new(0.0, 0.5);
        let mut b = TimeFlow::new(0.0, 0.5);
        a.pace = IDLE_PACE + WALK_PACE;
        b.pace = IDLE_PACE + 2.0 * WALK_PACE;
        let ya = run(&mut a, 30.0, 1.0);
        let yb = run(&mut b, 15.0, 2.0);
        assert!((ya - yb).abs() < 0.02, "{ya:.3} vs {yb:.3}");
    }

    #[test]
    fn pace_eases_instead_of_lurching() {
        let mut t = TimeFlow::new(0.0, 0.5);
        t.step(1.0 / 60.0, 1.0, false);
        assert!(t.days_per_sec() < 1.0, "the first frame of walking doesn't jump to full pace");
    }

    #[test]
    fn actions_time_lapse_their_cost_and_then_stop() {
        let mut t = TimeFlow::new(0.0, 0.5);
        let before = t.years();
        t.act(BUILD, "building");
        assert!(t.busy());
        assert_eq!(t.lapse().unwrap().0, "building");
        let passed = run(&mut t, BUILD.secs + 0.5, 0.0);
        assert!(!t.busy());
        assert!((passed - BUILD.years).abs() < 0.01, "{passed}");
        assert!((t.years() - before - passed).abs() < 1e-9);
    }

    #[test]
    fn a_lapse_blurs_the_days_resting_speeds_years() {
        let mut t = TimeFlow::new(0.0, 0.5);
        t.act(FELL, "felling");
        t.step(0.1, 0.0, false);
        assert!(crate::render::sky::blur_for_rate(t.days_per_sec()) > 0.99);
        let mut r = TimeFlow::new(0.0, 0.5);
        let mut y = 0.0;
        for _ in 0..300 {
            y += r.step(1.0 / 60.0, 0.0, true);
        }
        assert!(y > 3.0, "5 s resting passes years: {y:.2}");
    }
}
