//! Token Bleed Shield core library.
//!
//! Implements the Phase 0 vertical slice of `docs/architecture`:
//! - protocol normalization IR ([`ir`], [`adapter`])
//! - bounded-memory streaming primitives ([`scanner`], [`window`])
//! - L0 detectors D1–D5 ([`detectors`])
//! - zero-retention event guard + sinks ([`events`], [`sink`])
//! - counterfactual savings ledger ([`ledger`]) and policy compiler ([`policy`])

pub mod adapter;
pub mod detectors;
pub mod events;
pub mod hash;
pub mod ir;
pub mod ledger;
pub mod policy;
pub mod pricing;
pub mod scanner;
pub mod sink;
pub mod token_est;
pub mod window;
