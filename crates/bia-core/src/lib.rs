//! DSL types, JSON Schema, Expressions, validation and the `Action` trait.
//!
//! This crate performs no IO: no database, network, filesystem or clock reads.

pub mod action;
pub mod dsl;
pub mod expression;
