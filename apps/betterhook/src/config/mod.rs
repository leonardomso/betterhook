//! Config parsing, validation, and canonical AST.
//!
//! The public entry points are [`parse::parse_file`] and [`parse::parse_bytes`].
//! Both produce a [`schema::RawConfig`] which can be lowered to the typed
//! [`schema::Config`] via [`schema::RawConfig::lower`].

pub mod parse;
pub mod schema;

pub use parse::{Format, parse_bytes, parse_file};
pub use schema::{Config, Hook, IsolateSpec, Job, Meta, RawConfig, ToolPathScope};
