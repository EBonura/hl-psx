//! Behaviour pins for game rules: spec properties plus goldens recorded from
//! the current implementation, so a reimplementation can be checked against
//! observable behaviour. One file per item or item group.

pub mod setpiece_kit;

mod blood_trail;
mod damage_compass;
mod flashlight;
mod geiger;
mod local_nav;
mod nav_graph;
mod player_damage;
mod pushable_float;
mod rotating_door;
mod scripted_sequence;
mod talk_monster;
mod tau_cannon;
mod track_path;
mod tram_follower_ride;

#[cfg(test)]
mod apache;
#[cfg(test)]
mod garg;
#[cfg(test)]
mod mortar;
#[cfg(test)]
mod nihilanth;
#[cfg(test)]
mod osprey;
#[cfg(test)]
mod tank;
