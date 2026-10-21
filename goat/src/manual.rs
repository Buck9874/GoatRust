//! The manual (GGPlayerControllerGame.StartManual / UpdateManual, GGPlayerInputGame.
//! CheckManualActivation, GGTrickManual): back then forward within mManualKeyInterval starts it on
//! the ground (or on the next landing); the goat rears up ("Manual", manualBlendNode) and glides on
//! (Acceleration = facing x 30, no input movement) while W/S balance an unstable meter; every
//! mScoringInterval on the ground adds mScorePerInterval and keeps the combo alive. Landing after a
//! jump ends it ("manual" with the accumulated score); a meter stuck at an end for
//! mManualBreakLimit ragdolls the goat, which breaks the combo.

/// Default__GGPlayerControllerGame.
pub const KEY_INTERVAL: f32 = 0.08;
pub const PLAYER_FORCE: f32 = 20.0;
pub const BREAK_LIMIT: f32 = 0.7;
/// Default__GGTrickManual.
pub const SCORING_INTERVAL: f32 = 0.05;
pub const SCORE_PER_INTERVAL: i32 = 10;
/// The manual's NewAccel magnitude (uu/s^2).
pub const ACCEL: f32 = 30.0;

#[derive(Clone, Debug, Default)]
pub struct Manual {
    /// mIsDoingManual.
    pub active: bool,
    /// mCurrentManualValue (0..100, 50 = balanced) and mManualSpeed.
    pub value: f32,
    pub speed: f32,
    /// mManualBreakBuffer.
    pub break_buffer: f32,
    /// GGTrickManual: mTrickIsActive, mScoringCountdown, mAccumulatedScore.
    pub trick_active: bool,
    pub countdown: f32,
    pub score: i32,
    /// GGPlayerInputGame.mTimePressedDown (seconds) and mPerformedManualInput.
    pub pressed_down: f32,
    pub pending: bool,
}

/// What the controller should do this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    None,
    /// The meter stayed at an end too long: ragdoll (and break the combo).
    Fall,
}

impl Manual {
    /// CheckManualActivation: true when the back-then-forward input asks for a manual.
    pub fn check_input(&mut self, forward: f32, now: f32) -> bool {
        if forward < -0.8 && !self.active {
            self.pressed_down = now;
        }
        if forward > 0.8 && now - self.pressed_down < KEY_INTERVAL {
            self.pressed_down = f32::MIN;
            return true;
        }
        false
    }

    /// StartManual + GGTrickManual.OnManual(true).
    pub fn start(&mut self) {
        self.break_buffer = 0.0;
        self.value = 50.0;
        // Rand(1) > 0 is never true
        self.speed = -5.0;
        self.active = true;
        self.score = 0;
        self.countdown = SCORING_INTERVAL;
        self.trick_active = true;
    }

    /// StopManual: the trick's score (GGTrickManual reports it either way).
    pub fn stop(&mut self) -> i32 {
        self.active = false;
        self.trick_active = false;
        std::mem::take(&mut self.score)
    }

    /// One frame on the ground: AdjustManualSpeed (per input tick, scaled to 60 Hz), UpdateManual
    /// and GGTrickManual.TrickTick. Returns (outcome, score added this frame).
    pub fn tick(&mut self, forward: f32, dt: f32) -> (Outcome, i32) {
        self.speed += forward * PLAYER_FORCE * (dt * 60.0);
        self.speed += (50.0 - self.value) * -7.0 * dt;
        self.value = (self.value + self.speed * dt).clamp(0.0, 100.0);
        let mut out = Outcome::None;
        if self.value == 0.0 || self.value == 100.0 {
            self.break_buffer += dt;
            if self.break_buffer > BREAK_LIMIT {
                out = Outcome::Fall;
            }
        }
        let mut added = 0;
        if self.trick_active {
            self.countdown -= dt;
            if self.countdown <= 0.0 {
                self.score += SCORE_PER_INTERVAL;
                added = SCORE_PER_INTERVAL;
                self.countdown = SCORING_INTERVAL;
            }
        }
        (out, added)
    }
}

/// GGTrickManual: mTrickName "manual", GetTrickScore = the accumulated score.
pub fn trick(score: i32) -> crate::score::Trick {
    crate::score::Trick {
        id: crate::score::TrickId::Manual,
        name: "manual".into(),
        clean_name: "manual".into(),
        score,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_needs_back_then_forward_quickly() {
        let mut m = Manual { pressed_down: f32::MIN, ..Default::default() };
        assert!(!m.check_input(1.0, 0.0));
        m.check_input(-1.0, 1.0);
        assert!(m.check_input(1.0, 1.05));
        m.check_input(-1.0, 2.0);
        assert!(!m.check_input(1.0, 2.2));
    }

    #[test]
    fn unbalanced_meter_falls_and_scores_meanwhile() {
        let mut m = Manual::default();
        m.start();
        let (mut fell, mut total) = (false, 0);
        for _ in 0..600 {
            let (o, s) = m.tick(0.0, 1.0 / 60.0);
            total += s;
            if o == Outcome::Fall {
                fell = true;
                break;
            }
        }
        assert!(fell, "the meter tips over without input");
        assert!(total > 0 && total == m.score);
    }
}
