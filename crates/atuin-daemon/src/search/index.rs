//! The search index. A [`SearchIndex`] holds the commands that searches match against. A
//! [`SharedSearchIndex`] shares a [`SearchIndex`] between tasks.

use atuin_client::settings::Search;
use atuin_common::filter::OrFilter;

mod core;
mod shared;

pub use core::{IndexFilterMode, LoadedSearch, PendingSearch, SearchIndex};

pub use shared::{AddPermit, RebuildSettings, RebuiltIndex, SharedSearchIndex};

/// Settings for a [`SearchIndex`].
#[derive(Debug, Clone, PartialEq)]
pub struct SearchIndexSettings {
    /// The multipliers used when computing frecency.
    pub frecency_multipliers: FrecencyMultipliers,
    /// Controls which shells' commands are included.
    pub shells: OrFilter<Vec<String>>,
}

impl Default for SearchIndexSettings {
    /// The default frecency multipliers, with every shell included.
    fn default() -> Self {
        Self {
            frecency_multipliers: FrecencyMultipliers::default(),
            shells: OrFilter::all(),
        }
    }
}

/// Configurable multipliers for frecency calculation.
///
/// A multiplier of 0.0 disables that component, 1.0 is unchanged, 2.0 doubles weight.
/// Values like 0.5 reduce weight by half, 1.5 increases by 50%, etc.
///
/// Typically these come from the [settings](Search).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrecencyMultipliers {
    /// Multiplier for the recency component.
    recency: f64,
    /// Multiplier for the frequency component.
    frequency: f64,
    /// Overall multiplier for the final frecency score.
    frecency: f64,
}

impl From<&Search> for FrecencyMultipliers {
    fn from(search_settings: &Search) -> Self {
        // Clamp multipliers to non-negative values to prevent broken frecency ranking
        // (negative values would produce unexpected results when cast to u32)
        Self {
            recency: search_settings.recency_score_multiplier.max(0.0),
            frequency: search_settings.frequency_score_multiplier.max(0.0),
            frecency: search_settings.frecency_score_multiplier.max(0.0),
        }
    }
}

impl Default for FrecencyMultipliers {
    fn default() -> Self {
        Self::from(&Search::default())
    }
}
