//! In-memory token usage for this process (shown in the REPL and used to
//! verify prompt caching). Persistent usage lives on the relay (`/v1/usage`).

use std::sync::Mutex;

use crate::api::Usage;

/// Running totals since the agent was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct UsageTotals {
    /// API requests made.
    pub requests: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Tokens read from the prompt cache.
    pub cache_read_input_tokens: u64,
    /// Tokens written to the prompt cache.
    pub cache_creation_input_tokens: u64,
}

/// Thread-safe accumulator.
#[derive(Debug, Default)]
pub struct SessionUsage {
    totals: Mutex<UsageTotals>,
    last: Mutex<Option<Usage>>,
}

impl SessionUsage {
    /// Add one response's usage.
    pub fn record(&self, u: &Usage) {
        if let Ok(mut t) = self.totals.lock() {
            t.requests += 1;
            t.input_tokens += u.input_tokens;
            t.output_tokens += u.output_tokens;
            t.cache_read_input_tokens += u.cache_read_input_tokens;
            t.cache_creation_input_tokens += u.cache_creation_input_tokens;
        }
        if let Ok(mut l) = self.last.lock() {
            *l = Some(*u);
        }
    }

    /// Totals so far.
    pub fn totals(&self) -> UsageTotals {
        self.totals.lock().map(|t| *t).unwrap_or_default()
    }

    /// Usage of the most recent response.
    pub fn last(&self) -> Option<Usage> {
        self.last.lock().ok().and_then(|l| *l)
    }
}
