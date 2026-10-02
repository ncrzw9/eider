//! eider: a declarative pipeline compiler for DuckDB.
//!
//! A project is a directory of plain `SELECT` files. Each file declares a
//! relation's target state; eider owns every statement that writes to the
//! warehouse. Authors cannot write the statement that would break the layer
//! contract, so the contract holds by construction rather than by review.
//!
//! Dependencies are read from the SQL itself: every table reference in the
//! parse tree is an edge, so there is no templating language and a model is
//! valid DuckDB as written. Environments differ only in what each logical
//! catalog (`bronze`, `silver`, …) is attached to, never in the SQL.
//!
//! eider is one static binary with no dependency beyond its parser. Checks
//! run offline, so CI needs no database and no credentials.

pub mod check;
pub mod config;
pub mod project;
pub mod sql;
