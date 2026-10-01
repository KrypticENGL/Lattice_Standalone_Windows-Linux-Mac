//! Runtime observation: turning an ordinary C++ program into a stream of URR
//! events, without the user writing anything Lattice-specific.
//!
//! ```text
//! user source --libclang analysis--> instrumentation plan --rewrite--> instrumented source
//!     --(existing compiler, + lattice-runtime)--> executable --run--> events (JSON lines)
//!     --receiver--> model::Timeline  (the existing URR)
//! ```
//!
//! This is the *first milestone* of observation (see
//! `docs/runtime-observation-architecture.md`): heap objects created with `new`,
//! their scalar/pointer field writes, and `delete`. It plugs into the existing
//! [`Instrumenter`](crate::runtime::instrumentation::Instrumenter) seam and
//! changes nothing about how programs are compiled or run.
//!
//! Dependencies point one way: `observe` uses `model` and `runtime`; `model`
//! knows nothing of `observe`.

pub mod analysis;
pub mod discovery;
mod instrumenter;
pub mod libclang;
pub mod pipe;
pub mod receiver;
pub mod report;
pub mod rewrite;
mod service;

pub use instrumenter::{InstrumentationSummary, Observation, ObservingInstrumenter, Transport};
pub use service::{graph_at, view_at, ObservationService, ObservationStatus, StepView, DEFAULT_EVENT_LIMIT};
