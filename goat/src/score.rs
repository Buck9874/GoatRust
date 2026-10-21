//! Tricks, combos and score (GGTrickManager / GGComboManager / GGTrick* ports).

use crate::goat::Goat;
use bevy::prelude::*;

/// Values from Default__GGComboManager.
const COMBO_ALIVE_DURATION: f32 = 5.0;
const REUSE_DIMINISHING_FACTOR: f32 = 0.35;
const COMBO_BREAKER_DURATION: f32 = 5.0;
const MAX_FREQUENCY: u32 = 4;
/// Default__GGTrickRagdoll / GGTrickPerfectLanding / GGTrickAbility.
const RAGDOLL_INTERVAL: f32 = 0.5;
const PERFECT_LANDING_SCORE: i32 = 750;
pub const PERFECT_LANDING_JUMP_TIME: f32 = 0.2;
const AIR_MULTIPLIER: f32 = 1.2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrickId {
    Ability,
    Ragdoll,
    PerfectLanding,
    Rotation,
    JumpOver,
    WallRun,
    WallJump,
    /// GGTrickKismet (GGSeqAct_GiveScore in the map's Kismet).
    Kismet,
    /// GGTrickFracturingApex ("Broke <name>").
    FracturingApex,
    /// GGTrickExplosion (10, no multiplier, no name).
    Explosion,
    /// GGTrickHurtByExplosion.
    HurtByExplosion,
    /// GGTrickCollision (a prop in the goat's collision chain hit something: its score, unnamed).
    Collision,
    /// GGTrickCollisionSpecial ("Skillshot").
    CollisionSpecial,
    /// GGTrickBounce (jump from/on trampolines).
    Bounce,
    /// GGTrickManual.
    Manual,
    /// GGTrickBaseball ("Beanball at ...").
    Baseball,
}

/// Tricks completed this frame, for NPC reactions (GGAIController.OnTrickMade).
#[derive(Resource, Default)]
pub struct TrickEvents(pub Vec<TrickId>);

#[derive(Clone, Debug)]
pub struct Trick {
    pub id: TrickId,
    /// Display name (e.g. "headbutt Fence").
    pub name: String,
    /// Name used to find repeats in a combo (e.g. "headbutt").
    pub clean_name: String,
    pub score: i32,
    pub adds_multiplier: bool,
    pub has_name: bool,
    pub min_score: i32,
}

#[derive(Clone, Debug)]
struct TrickInfo {
    id: TrickId,
    clean_name: String,
    has_name: bool,
    adds_multiplier: bool,
    frequency: u32,
    trick_score: i32,
}

/// One HUD line of the running combo.
#[derive(Clone, Debug)]
pub struct ComboLine {
    pub text: String,
    pub score: i32,
    pub age: f32,
}

/// What GGComboManager reports to the HUD movie (GGHUDGfxInGame), in order.
#[derive(Clone, Debug)]
pub enum HudEvent {
    /// ReportTrickToHUD -> AddCombo(trickInf, trickScore, CalculateTotalScore(false), mMultiplier,
    /// scoreIsEligable, displayName).
    Trick { name: String, clean_name: String, has_name: bool, frequency: u32, trick_score: i32, score: i32, combo_score: i32, multiplier: f32, eligible: bool },
    /// EndCurrentCombo -> ReportComboEndToHUD (EndCombo, UpdateScore) with the new total.
    ComboEnd { total: i64, combo: i32 },
    /// BreakCombo -> mHUDMovie.BreakCombo(message).
    BreakCombo(String),
    /// The manual widget: StartManual / StopManual, UpdatePluppPosition(value) + UpdateManualScore.
    ManualStart,
    ManualStop,
    Manual { value: f32, score: i32 },
}

#[derive(Resource, Default)]
pub struct Score {
    pub total: i64,
    pub multiplier: f32,
    combo: Vec<TrickInfo>,
    since_last: f32,
    pub lines: Vec<ComboLine>,
    /// Last finished combo (for the HUD flash).
    pub last_combo: Option<(i32, f32)>,
    ragdoll_timer: f32,
    /// Tricks reported since the last `take_made` (feeds TrickEvents).
    made: Vec<TrickId>,
    /// For the Scaleform HUD (drained by uvm::menus; capped when nothing reads it).
    pub hud: Vec<HudEvent>,
    /// Combos ended or broken so far (GGGameInfo.OnEndCombo resets the collision chains).
    pub combos_ended: u32,
    /// mComboBreakerCountdown: no new combo while > 0.
    breaker: f32,
}

impl Score {
    pub fn report(&mut self, t: Trick) {
        // mComboBreakerActive: a new combo can't start right after one was broken
        if self.breaker > 0.0 {
            return;
        }
        self.made.push(t.id);
        let idx = self.combo.iter().position(|c| (t.has_name && c.clean_name == t.clean_name) || (!t.has_name && c.id == t.id));
        let idx = match idx {
            Some(i) => i,
            None => {
                self.combo.push(TrickInfo {
                    id: t.id,
                    clean_name: t.clean_name.clone(),
                    has_name: t.has_name,
                    adds_multiplier: t.adds_multiplier,
                    frequency: 0,
                    trick_score: 0,
                });
                self.combo.len() - 1
            }
        };
        let freq = self.combo[idx].frequency + 1;
        // GetAdjustedScore: repeats are worth 0.35^(n-1)
        let factor = if freq > 1 { REUSE_DIMINISHING_FACTOR.powi(freq as i32 - 1) } else { 1.0 };
        let score = ((t.score as f32 * factor) as i32).max(t.min_score);
        if (t.has_name && t.name.is_empty()) || score <= 0 {
            return;
        }
        self.combo[idx].frequency = freq;
        self.combo[idx].trick_score += score;
        self.update_multiplier();
        let c = &self.combo[idx];
        let ev = HudEvent::Trick {
            name: t.name.clone(),
            clean_name: t.clean_name.clone(),
            has_name: c.has_name,
            frequency: c.frequency,
            trick_score: c.trick_score,
            score,
            combo_score: self.combo.iter().map(|c| c.trick_score).sum(),
            multiplier: self.multiplier,
            eligible: c.frequency <= MAX_FREQUENCY || !c.has_name,
        };
        self.push_hud(ev);
        // unnamed tricks (GetTrickName "") only add to the score
        if !t.name.is_empty() {
            self.lines.push(ComboLine { text: t.name, score, age: 0.0 });
            if self.lines.len() > 8 {
                self.lines.remove(0);
            }
        }
    }

    /// GGComboManager.BreakCombo (+ GGGameInfo.OnBreakCombo's chain reset): the combo is lost.
    pub fn break_combo(&mut self, message: &str) {
        self.multiplier = 0.0;
        self.combo.clear();
        self.lines.clear();
        self.since_last = 0.0;
        self.breaker = COMBO_BREAKER_DURATION;
        self.combos_ended += 1;
        self.push_hud(HudEvent::BreakCombo(message.to_string()));
    }

    /// mTimeSinceLastScoreAdded = 0 (GGTrickManual keeps the combo going).
    pub fn keep_alive(&mut self) {
        self.since_last = 0.0;
    }

    /// For the manual widget and other HUD-only notifications.
    pub fn hud_event(&mut self, ev: HudEvent) {
        self.push_hud(ev);
    }

    fn update_multiplier(&mut self) {
        let prev = self.multiplier;
        let n: u32 = self.combo.iter().filter(|c| c.adds_multiplier).map(|c| c.frequency.min(MAX_FREQUENCY)).sum::<u32>().max(1);
        let mut m = 0.0f32;
        for _ in 0..n {
            let added = (m / 10.0).floor();
            m += 1.0 / (added + 1.0);
        }
        self.multiplier = m;
        if prev != m {
            self.since_last = 0.0;
        }
    }

    fn push_hud(&mut self, ev: HudEvent) {
        if self.hud.len() > 256 {
            self.hud.drain(..128);
        }
        self.hud.push(ev);
    }

    /// GGComboManager.mTimeSinceLastScoreAdded / mComboAliveDuration (the HUD's combo bar).
    pub fn combo_timer(&self) -> (f32, f32) {
        (self.since_last, COMBO_ALIVE_DURATION)
    }

    pub fn combo_total(&self) -> i32 {
        let sum: i32 = self.combo.iter().map(|c| c.trick_score).sum();
        (sum as f32 * self.multiplier) as i32
    }

    pub fn in_combo(&self) -> bool {
        !self.combo.is_empty()
    }

    fn tick(&mut self, dt: f32) {
        self.breaker = (self.breaker - dt).max(0.0);
        if !self.combo.is_empty() {
            self.since_last += dt;
        }
        if self.since_last > COMBO_ALIVE_DURATION && !self.combo.is_empty() {
            let total = self.combo_total();
            self.total += total as i64;
            self.last_combo = Some((total, 0.0));
            let combo = total;
            let total = self.total;
            self.push_hud(HudEvent::ComboEnd { total, combo });
            self.combos_ended += 1;
            self.combo.clear();
            self.lines.clear();
            self.multiplier = 0.0;
            self.since_last = 0.0;
        }
        for l in &mut self.lines {
            l.age += dt;
        }
        if let Some((_, age)) = &mut self.last_combo {
            *age += dt;
            if *age > 3.0 {
                self.last_combo = None;
            }
        }
    }
}

/// Default__GGTrickRotation values.
const SPIN_180_SCORE: f32 = 10.0;
const FLIP_SCORE: f32 = 50.0;
const SPIN_MULTIPLIER: f32 = 1.25;
const FLIP_MULTIPLIER: f32 = 1.5;
const SPIN_AND_FLIP_MULTIPLIER: f32 = 1.5;
const LICK_ROTATION_MULTIPLIER: f32 = 1.5;
const FLIP_NAMES: [&str; 11] =
    ["", "Double", "Triple", "Quadruple", "Pentuple", "Hextuple", "Septuple", "Octuple", "Nonuple", "Decuple", "Integer overfloweduple"];

/// GGTrickRotation.CalculateFinalRotation + GetTrickName/GetTrickScore. `yaw_deg` / `pitch_deg`
/// are the signed rotations accumulated in the air (positive pitch = nose up = backflip).
/// `licked` is the name of an item held in the mouth while rotating.
pub fn rotation_trick(yaw_deg: f32, pitch_deg: f32, licked: Option<&str>) -> Option<Trick> {
    let mut halves = (yaw_deg / 180.0) as i32;
    let yaw_rest = (yaw_deg % 180.0) as i32;
    if yaw_rest.abs() >= 170 {
        halves += if yaw_rest < 0 { -1 } else { 1 };
    }
    let mut flips = (pitch_deg / 360.0) as i32;
    let pitch_rest = (pitch_deg % 360.0) as i32;
    if pitch_rest.abs() > 300 {
        flips += if pitch_rest < 0 { -1 } else { 1 };
    }
    if halves == 0 && flips == 0 {
        return None;
    }
    let n180 = halves.unsigned_abs();
    let nflips = (flips.unsigned_abs() as usize).min(FLIP_NAMES.len());
    // name
    let spin = if n180 > 0 { (n180 * 180).to_string() } else { String::new() };
    let mut name = String::new();
    if nflips > 0 {
        let flip = if flips > 0 { "BackFlip" } else { "FrontFlip" };
        name = format!("{} {flip}", FLIP_NAMES[nflips - 1]);
    }
    name = match licked {
        Some(l) => format!("{spin} {l}- lick {name}"),
        None => format!("{spin} {name}"),
    };
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    // score
    let mut spin_score = if n180 > 0 { SPIN_180_SCORE } else { 0.0 };
    for _ in 1..n180 {
        spin_score += SPIN_180_SCORE;
        spin_score *= SPIN_MULTIPLIER;
    }
    let mut flip_score = if nflips > 0 { FLIP_SCORE } else { 0.0 };
    for _ in 1..nflips {
        flip_score += FLIP_SCORE;
        flip_score *= FLIP_MULTIPLIER;
    }
    let mut total = spin_score + flip_score;
    if nflips as u32 + n180 >= 2 {
        total *= SPIN_AND_FLIP_MULTIPLIER;
    }
    if licked.is_some() {
        total *= LICK_ROTATION_MULTIPLIER;
    }
    Some(Trick {
        id: TrickId::Rotation,
        clean_name: name.clone(),
        name,
        score: total as i32,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    })
}

/// GGTrickJumpOver (mScorePctJumpedOverActor = 1).
pub fn jump_over(actor_name: &str, actor_score: i32) -> Trick {
    Trick {
        id: TrickId::JumpOver,
        name: format!("jumped over {actor_name}"),
        clean_name: "jumped over".into(),
        score: actor_score,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    }
}

/// GGTrickWallRun: 75 after 0.7 s on the wall.
pub const WALL_RUN_MIN_TIME: f32 = 0.7;
pub fn wall_run() -> Trick {
    Trick { id: TrickId::WallRun, name: "Wall Run".into(), clean_name: "Wall Run".into(), score: 75, adds_multiplier: true, has_name: true, min_score: 1 }
}

/// GGTrickWallJump: 50.
pub fn wall_jump() -> Trick {
    Trick { id: TrickId::WallJump, name: "Wall Jump".into(), clean_name: "Wall Jump".into(), score: 50, adds_multiplier: true, has_name: true, min_score: 1 }
}

/// Ability trick (GGTrickAbility.OnUseAbility) for a scored victim.
pub fn ability_trick(ability: &str, in_air: bool, victim_score: i32, victim_name: &str) -> Trick {
    let mut clean = ability.to_string();
    let mut score = victim_score;
    if in_air {
        clean = format!("air {clean}");
        score = (score as f32 * AIR_MULTIPLIER) as i32;
    }
    Trick {
        id: TrickId::Ability,
        name: format!("{clean} {victim_name}"),
        clean_name: clean,
        score,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    }
}

pub fn perfect_landing() -> Trick {
    Trick {
        id: TrickId::PerfectLanding,
        name: "Perfect Landing".into(),
        clean_name: "Perfect Landing".into(),
        score: PERFECT_LANDING_SCORE,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    }
}

pub fn tick(time: Res<Time>, mut score: ResMut<Score>, mut events: ResMut<TrickEvents>, goats: Query<&Goat>) {
    let _prof_scope = crate::prof::scope("score::tick");
    let dt = time.delta_secs();
    events.0.append(&mut score.made);
    // GGTrickRagdoll: 1 point every 0.5 s while ragdolled (no name, no multiplier)
    if goats.iter().any(|g| g.ragdoll) {
        score.ragdoll_timer += dt;
        while score.ragdoll_timer >= RAGDOLL_INTERVAL {
            score.ragdoll_timer -= RAGDOLL_INTERVAL;
            score.report(Trick {
                id: TrickId::Ragdoll,
                name: "Ragdoll".into(),
                clean_name: String::new(),
                score: 1,
                adds_multiplier: false,
                has_name: false,
                min_score: 1,
            });
        }
    } else {
        score.ragdoll_timer = 0.0;
    }
    score.tick(dt);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplier_matches_ggcombomanager() {
        let mut s = Score::default();
        s.report(ability_trick("headbutt", false, 10, "Fence"));
        assert_eq!(s.multiplier, 1.0);
        s.report(ability_trick("lick", false, 10, "Fence"));
        assert_eq!(s.multiplier, 2.0);
        // repeat of the same clean name decays: 10 * 0.35 = 3
        s.report(ability_trick("headbutt", false, 10, "Fence"));
        assert_eq!(s.combo[0].trick_score, 13);
        assert_eq!(s.multiplier, 3.0);
    }

    #[test]
    fn rotation_names_and_scores() {
        // one backflip
        let t = rotation_trick(0.0, 365.0, None).unwrap();
        assert_eq!(t.name, "BackFlip");
        assert_eq!(t.score, 50);
        // 360 spin = two 180s: (10 + 10) * 1.25 = 25, then x1.5 for >= 2 rotations
        let t = rotation_trick(-362.0, 0.0, None).unwrap();
        assert_eq!(t.name, "360");
        assert_eq!(t.score, 37);
        // 330 deg of front flip rounds up to one flip (rest > 300)
        let t = rotation_trick(0.0, -330.0, None).unwrap();
        assert_eq!(t.name, "FrontFlip");
        // two backflips: (50 + 50) * 1.5 = 150, x1.5 = 225
        let t = rotation_trick(0.0, 725.0, None).unwrap();
        assert_eq!(t.name, "Double BackFlip");
        assert_eq!(t.score, 225);
        assert!(rotation_trick(100.0, 200.0, None).is_none());
    }

    #[test]
    fn combo_banks_after_alive_duration() {
        let mut s = Score::default();
        s.report(ability_trick("headbutt", false, 50, "Beacon"));
        s.tick(5.1);
        assert_eq!(s.total, 50);
        assert!(!s.in_combo());
    }
}

/// GGTrickFracturingApex.OnFractured: "Broke" + the destructible's name, its physmat score.
pub fn fractured(actor_name: &str, actor_score: i32) -> Trick {
    Trick {
        id: TrickId::FracturingApex,
        name: format!("Broke {actor_name}"),
        clean_name: "Broke".into(),
        score: actor_score,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    }
}

/// GGTrickExplosion (Default__: mTrickScore 10, mAddsMultiplier false, mHasAName false).
pub fn explosion() -> Trick {
    Trick { id: TrickId::Explosion, name: "Explosion".into(), clean_name: "Explosion".into(), score: 10, adds_multiplier: false, has_name: false, min_score: 1 }
}

/// GGTrickCollision (Default__: mAddsMultiplier false, mHasAName false): the hitter's score.
pub fn collision(score: i32) -> Trick {
    Trick { id: TrickId::Collision, name: String::new(), clean_name: String::new(), score, adds_multiplier: false, has_name: false, min_score: 1 }
}

/// GGTrickCollisionSpecial: mTrickTitle "Skillshot", travelled distance / 100 (mMinTravelDistance
/// 2000 uu since the prop woke).
pub fn skillshot(travel_uu: f32) -> Option<Trick> {
    (travel_uu >= 2000.0).then(|| Trick {
        id: TrickId::CollisionSpecial,
        name: "Skillshot".into(),
        clean_name: "Skillshot".into(),
        score: (travel_uu / 100.0) as i32,
        adds_multiplier: true,
        has_name: true,
        min_score: 1,
    })
}

/// What the goat stood or bounced on, for GGTrickBounce.FindMatchingPair.
#[derive(Clone, Debug, PartialEq)]
pub enum BelowFeet {
    /// Any other actor (ground, props).
    Other,
    /// A GGTrampolineVolume: (index, child of GGTrampolineVolumeTrampolineAbstract, mDisplayName).
    Volume(usize, bool, String),
}

/// GGTrickBounce mJumpPairInfos (Default__GGTrickBounce): trampoline -> other trampoline 500,
/// bouncy volume -> other bouncy volume 250 (both "jump from [FROM] to [TO]", add multiplier),
/// anything -> trampoline 50, anything -> bouncy volume 25 ("jump on [TO]").
pub fn bounce(from: &BelowFeet, to: &BelowFeet) -> Option<Trick> {
    let BelowFeet::Volume(ti, t_tramp, t_name) = to else { return None };
    let pair = match from {
        BelowFeet::Volume(fi, f_tramp, f_name) if fi != ti => match (*f_tramp, *t_tramp) {
            (true, true) => Some((format!("jump from {f_name} to {t_name}"), 500)),
            _ => Some((format!("jump from {f_name} to {t_name}"), 250)),
        },
        _ => None,
    };
    let (name, score, mult) = match pair {
        Some((n, s)) => (n, s, true),
        None => (format!("jump on {t_name}"), if *t_tramp { 50 } else { 25 }, false),
    };
    Some(Trick { id: TrickId::Bounce, clean_name: name.clone(), name, score, adds_multiplier: mult, has_name: true, min_score: 1 })
}

#[cfg(test)]
mod bounce_tests {
    use super::*;

    #[test]
    fn jump_pairs_match_ggtrickbounce() {
        let t1 = BelowFeet::Volume(1, true, "Trampoline".into());
        let t2 = BelowFeet::Volume(2, true, "Trampoline".into());
        let m = BelowFeet::Volume(3, false, "Mattress".into());
        assert_eq!(bounce(&t1, &t2).unwrap().score, 500);
        assert_eq!(bounce(&m, &t1).unwrap().score, 250);
        assert_eq!(bounce(&t1, &t1).unwrap().score, 50);
        assert_eq!(bounce(&BelowFeet::Other, &m).unwrap().name, "jump on Mattress");
        assert!(!bounce(&BelowFeet::Other, &m).unwrap().adds_multiplier);
        assert!(bounce(&t1, &BelowFeet::Other).is_none());
        assert!(skillshot(1999.0).is_none());
        assert_eq!(skillshot(3500.0).unwrap().score, 35);
    }
}
