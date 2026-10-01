//! Source metadata attached to events and frames. Plain data; the model never
//! reads the file.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A position in the user's source. `line`/`column` are 1-based. The file and
/// function names are `Arc<str>` so the thousands of events from one location
/// share a single allocation (an observer should reuse the same `Arc`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: Arc<str>,
    pub line: u32,
    pub column: Option<u32>,
    pub function: Option<Arc<str>>,
    /// Instruction address, when the observer works at that level.
    pub instruction: Option<u64>,
}

impl SourceLocation {
    pub fn new(file: impl Into<Arc<str>>, line: u32) -> Self {
        SourceLocation { file: file.into(), line, column: None, function: None, instruction: None }
    }

    pub fn with_column(mut self, column: u32) -> Self {
        self.column = Some(column);
        self
    }

    pub fn with_function(mut self, function: impl Into<Arc<str>>) -> Self {
        self.function = Some(function.into());
        self
    }
}
