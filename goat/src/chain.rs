//! Collision chains (GGCollidableActorInterface): GGGameInfo.UpdateCollisionChainWithGoat numbers
//! every actor by how many collisions away from the goat it is (the goat is 0, a prop it
//! headbutts, bites or bumps is 1, what that prop hits is 2, ...), and the tricks fed by
//! GGGameInfo.OnCollision: GGTrickCollision (a chained prop hitting anything scores its own
//! GetScore, unnamed, no multiplier) and GGTrickCollisionSpecial ("Skillshot": a chained prop that
//! travelled 2000 uu since it woke hits a score actor). Chains reset when a prop sleeps
//! (OnSleepRBPhysics) and when the combo ends (OnEndCombo).

use crate::convert;
use crate::goat::Goat;
use crate::physics::Physics;
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::collections::HashMap;

/// GGKActorBase.mMinTimeBetweenCollisions.
const MIN_TIME_BETWEEN_COLLISIONS: f32 = 1.0;
/// Default__GGNpc.mMinTimeBetweenCollisions.
const NPC_MIN_TIME_BETWEEN_COLLISIONS: f32 = 2.0;

#[derive(Resource, Default)]
pub struct Chains {
    /// mCollisionChainGoatNr of props (absent = 0, not in a chain).
    props: HashMap<rp::RigidBodyHandle, u32>,
    /// ... and of NPCs.
    npcs: HashMap<Entity, u32>,
    /// mWakeRBLocation of awake props (Bevy space).
    wake: HashMap<rp::RigidBodyHandle, Vec3>,
    /// mLastCollisionData.CollisionTimestamp.
    last: HashMap<rp::RigidBodyHandle, f32>,
    npc_last: HashMap<Entity, f32>,
    /// GGKActorBaseBallAbstract bodies and whether they are lit (IsOnFire).
    baseballs: HashMap<rp::RigidBodyHandle, bool>,
    /// Beanballs so far (tests).
    pub beanballs: u32,
    combos_seen: u32,
    /// Collision / Skillshot tricks so far (tests).
    pub collisions: u32,
    pub skillshots: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Actor {
    Goat,
    Prop(rp::RigidBodyHandle),
    Npc(Entity),
    /// Not a GGCollidableActorInterface (BSP, static meshes, landscape).
    World,
}

impl Chains {
    /// (collidable, chain nr); the goat is always in the chain with nr 0.
    fn nr(&self, a: Actor) -> Option<u32> {
        match a {
            Actor::Goat => Some(0),
            Actor::Prop(h) => Some(self.props.get(&h).copied().unwrap_or(0)),
            Actor::Npc(e) => Some(self.npcs.get(&e).copied().unwrap_or(0)),
            Actor::World => None,
        }
    }

    fn in_chain(&self, a: Actor) -> bool {
        a == Actor::Goat || self.nr(a).is_some_and(|n| n > 0)
    }

    /// SetCollisionChainGoatNr(other) on `a`.
    fn set_from(&mut self, a: Actor, other: Actor) {
        let (Some(mine), Some(theirs)) = (self.nr(a), self.nr(other)) else { return };
        if !self.in_chain(other) || (theirs >= mine && mine > 0) {
            return;
        }
        match a {
            Actor::Prop(h) => {
                self.props.insert(h, theirs + 1);
            }
            Actor::Npc(e) => {
                self.npcs.insert(e, theirs + 1);
            }
            Actor::Goat | Actor::World => {}
        }
    }

    pub fn add_baseball(&mut self, h: rp::RigidBodyHandle, lit: bool) {
        self.baseballs.retain(|_, _| true);
        if self.baseballs.len() > 256 {
            self.baseballs.clear();
        }
        self.baseballs.insert(h, lit);
    }

    /// GGGoat abilities (headbutt, kick, bite) put what they hit in the chain.
    pub fn touched_by_goat(&mut self, h: rp::RigidBodyHandle) {
        self.set_from(Actor::Prop(h), Actor::Goat);
    }
}

/// The goat knocking an NPC over (headbutt, running into it, its abilities, its explosions) puts
/// the NPC in its chain.
fn npc_knocked(ev: On<Insert, crate::npc::Knocked>, mut chains: ResMut<Chains>) {
    chains.set_from(Actor::Npc(ev.entity), Actor::Goat);
}

#[allow(clippy::type_complexity)]
pub fn chains(
    time: Res<Time>,
    mut chains: ResMut<Chains>,
    phys: Res<Physics>,
    mut score: ResMut<crate::score::Score>,
    goats: Query<(&Goat, Option<&crate::ragdoll::ActiveRagdoll>)>,
    npcs: Query<(Entity, &crate::npc::Npc, Option<&crate::ragdoll::ActiveRagdoll>)>,
) {
    let _prof_scope = crate::prof::scope("chain::chains");
    let c = &mut *chains;
    let now = time.elapsed_secs();
    if score.combos_ended != c.combos_seen {
        c.combos_seen = score.combos_ended;
        c.props.clear();
        c.npcs.clear();
    }
    // OnWakeRBPhysics / OnSleepRBPhysics
    c.wake.retain(|h, _| phys.world.bodies.get(*h).is_some_and(|b| b.is_dynamic() && !b.is_sleeping()));
    let props = &mut c.props;
    props.retain(|h, _| phys.world.bodies.get(*h).is_some_and(|b| b.is_dynamic() && !b.is_sleeping()));
    for h in phys.world.islands.active_bodies() {
        if phys.prop_info.contains_key(&h) && !c.wake.contains_key(&h) {
            if let Some(b) = phys.world.bodies.get(h) {
                c.wake.insert(h, convert::from_rapier(b.translation()));
            }
        }
    }
    let Ok((g, rd)) = goats.single() else { return };
    // headbutts and other abilities from the goat
    for hit in &phys.hits {
        if hit.goat.is_none() {
            continue;
        }
        if let Some(p) = phys.world.colliders.get(hit.collider).and_then(|col| col.parent()) {
            if phys.prop_info.contains_key(&p) {
                c.touched_by_goat(p);
            }
        }
    }
    // what's bitten stays in the chain (GGGoat: SetCollisionChainGoatNr on the grabbed item)
    if let Some((h, _)) = g.grabbed {
        c.touched_by_goat(h);
    }
    let goat_body = |b: rp::RigidBodyHandle| b == g.body || rd.is_some_and(|rd| rd.bodies.iter().any(|&(_, h)| h == b));
    let npc_bodies: HashMap<rp::RigidBodyHandle, Entity> = npcs.iter().filter_map(|(e, _, r)| r.map(|r| (e, r))).flat_map(|(e, r)| r.bodies.iter().map(move |&(_, h)| (h, e))).collect();
    let actor_of = |col: rp::ColliderHandle| -> Actor {
        if let Some(&e) = phys.owners.get(&col) {
            return Actor::Npc(e);
        }
        match phys.world.colliders.get(col).and_then(|x| x.parent()) {
            Some(b) if goat_body(b) => Actor::Goat,
            Some(b) if npc_bodies.contains_key(&b) => Actor::Npc(npc_bodies[&b]),
            Some(b) if phys.prop_info.contains_key(&b) => Actor::Prop(b),
            _ => Actor::World,
        }
    };
    let grabbed = g.grabbed.map(|(h, _)| h);
    for &(h, other_col, pos) in &phys.contacts {
        let other = actor_of(other_col);
        // GGKActorBase.Collided: not again within mMinTimeBetweenCollisions, not while bitten
        if grabbed == Some(h) && other == Actor::Goat {
            continue;
        }
        if c.last.get(&h).is_some_and(|&t| now - t < MIN_TIME_BETWEEN_COLLISIONS) {
            continue;
        }
        c.last.insert(h, now);
        let me = Actor::Prop(h);
        // UpdateCollisionChainWithGoat, then the tricks (GGTrickManager.OnCollision)
        c.set_from(me, other);
        c.set_from(other, me);
        if !c.in_chain(me) {
            continue;
        }
        // GGTrickBaseball: a goat's baseball hitting a person
        if let (Some(&lit), Actor::Npc(ne)) = (c.baseballs.get(&h), other) {
            if let Ok((_, n, _)) = npcs.get(ne) {
                let clean = if lit { "Flaming Beanball" } else { "Beanball" };
                score.report(crate::score::Trick {
                    id: crate::score::TrickId::Baseball,
                    name: format!("{clean} at {}", n.actor_name),
                    clean_name: clean.into(),
                    score: 100 + n.score,
                    adds_multiplier: true,
                    has_name: true,
                    min_score: 1,
                });
                c.beanballs += 1;
            }
        }
        let (prop_score, _) = phys.prop_info.get(&h).cloned().unwrap_or_default();
        if prop_score > 0 {
            score.report(crate::score::collision(prop_score));
            c.collisions += 1;
        }
        if matches!(other, Actor::Prop(_) | Actor::Npc(_)) {
            let travel = c.wake.get(&h).map(|w| w.distance(pos) / convert::S).unwrap_or(0.0);
            if let Some(t) = crate::score::skillshot(travel) {
                score.report(t);
                c.skillshots += 1;
            }
        }
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    chain: {} nr {} hit {:?}", phys.names.get(&h).cloned().unwrap_or_default(), c.props.get(&h).copied().unwrap_or(0), c.nr(other));
        }
    }
    // GGNpc.RigidBodyCollision -> Collided while ragdolled: a chained NPC scores its GetScore
    const THRESHOLD: f32 = 50.0 * convert::S;
    for (e, n, r) in &npcs {
        let Some(r) = r else { continue };
        if !c.in_chain(Actor::Npc(e)) || c.npc_last.get(&e).is_some_and(|&t| now - t < NPC_MIN_TIME_BETWEEN_COLLISIONS) {
            continue;
        }
        let hit = r.bodies.iter().find_map(|&(_, h)| {
            let rb = phys.world.bodies.get(h)?;
            let mass = rb.mass().max(0.1);
            rb.colliders().iter().find_map(|&col| {
                phys.world.narrow_phase.contact_pairs_with(col).find_map(|pair| {
                    let other = if pair.collider1 == col { pair.collider2 } else { pair.collider1 };
                    let other_actor = actor_of(other);
                    (pair.has_any_active_contact() && pair.total_impulse_magnitude() / mass >= THRESHOLD && other_actor != Actor::Npc(e)).then_some(other_actor)
                })
            })
        });
        let Some(other) = hit else { continue };
        c.npc_last.insert(e, now);
        c.set_from(Actor::Npc(e), other);
        c.set_from(other, Actor::Npc(e));
        if n.score > 0 {
            score.report(crate::score::collision(n.score));
            c.collisions += 1;
        }
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    chain: npc {} nr {} hit {:?}", n.actor_name, c.npcs.get(&e).copied().unwrap_or(0), c.nr(other));
        }
    }
    // forget timestamps of props that went back to sleep
    if c.last.len() > 256 {
        c.last.retain(|_, &mut t| now - t < MIN_TIME_BETWEEN_COLLISIONS);
    }
    if c.npc_last.len() > 256 {
        c.npc_last.retain(|_, &mut t| now - t < NPC_MIN_TIME_BETWEEN_COLLISIONS);
    }
}

pub struct ChainPlugin;

impl Plugin for ChainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Chains>().add_observer(npc_knocked).add_systems(PostUpdate, chains.after(crate::physics::step));
    }
}
