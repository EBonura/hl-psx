//! Behaviour tests for game modules whose bodies are due to be rewritten.
//! Spec-property tests assert the rules a rewrite must keep; golden tests
//! replay a fixed scenario and compare every output against a table
//! recorded from the current behaviour.

pub mod setpiece_rng;

#[cfg(test)]
mod mortar;
