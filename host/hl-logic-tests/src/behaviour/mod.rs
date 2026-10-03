//! Behaviour tests for game modules whose bodies are due to be rewritten.
//! Spec-property tests assert the rules a rewrite must keep; golden tests
//! replay a fixed scenario and compare every output against a table
//! recorded from the current behaviour.

pub mod setpiece_kit;

#[cfg(test)]
mod apache;
#[cfg(test)]
mod garg;
#[cfg(test)]
mod mortar;
#[cfg(test)]
mod osprey;
#[cfg(test)]
mod tank;
