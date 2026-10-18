//! DSL types, JSON Schema, Expressions, validation, the `Action` trait and the `Clock` trait.
//!
//! This crate performs no IO: no database, network, filesystem or clock reads.

pub mod action;
pub mod clock;
pub mod dsl;
pub mod expression;
pub mod validation;
