//! Versioned price tables (docs/architecture/05 §1.3): customer-supplied table
//! with provider-list defaults; cost arithmetic for the counterfactual ledger.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read_per_mtok: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriceTableFile {
    #[serde(default = "default_version")]
    pub version: u16,
    pub models: BTreeMap<String, ModelPrice>,
}

fn default_version() -> u16 {
    1
}

#[derive(Debug, Clone)]
pub struct PriceTable {
    pub version: u16,
    pub models: BTreeMap<String, ModelPrice>,
}

impl Default for PriceTable {
    fn default() -> Self {
        Self::provider_defaults()
    }
}

impl PriceTable {
    /// Provider-list defaults (docs/architecture/05 §3.2 worked example family).
    pub fn provider_defaults() -> Self {
        let mut models = BTreeMap::new();
        let mut add = |name: &str, input: f64, output: f64| {
            models.insert(
                name.to_string(),
                ModelPrice {
                    input_per_mtok: input,
                    output_per_mtok: output,
                    cache_read_per_mtok: input * 0.5,
                },
            );
        };
        add("gpt-4o", 2.50, 10.00);
        add("gpt-4o-mini", 0.15, 0.60);
        add("gpt-4.1", 2.00, 8.00);
        add("gpt-4.1-mini", 0.40, 1.60);
        add("o3-mini", 1.10, 4.40);
        add("claude-sonnet-4", 3.00, 15.00);
        add("claude-opus-4", 15.00, 75.00);
        add("claude-3-5-haiku", 0.80, 4.00);
        add("claude-3-5-sonnet", 3.00, 15.00);
        add("gemini-1.5-pro", 1.25, 5.00);
        add("gemini-1.5-flash", 0.075, 0.30);
        Self { version: 1, models }
    }

    pub fn from_file(file: PriceTableFile) -> Self {
        Self {
            version: file.version,
            models: file.models,
        }
    }

    /// Longest-prefix match so dated model ids (`gpt-4o-2024-08-06`) resolve.
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice> {
        if let Some(exact) = self.models.get(model) {
            return Some(exact);
        }
        self.models
            .iter()
            .filter(|(name, _)| model.starts_with(name.as_str()))
            .max_by_key(|(name, _)| name.len())
            .map(|(_, price)| price)
    }

    /// (cost_usd, price_known)
    pub fn cost(&self, model: &str, input_tokens: u32, output_tokens: u32) -> (f64, bool) {
        self.cost_with_cache(model, input_tokens, output_tokens, 0)
    }

    pub fn cost_with_cache(
        &self,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
    ) -> (f64, bool) {
        let Some(price) = self.lookup(model) else {
            return (0.0, false);
        };
        let billable_input = input_tokens.saturating_sub(cache_read_tokens) as f64;
        let cost = billable_input * price.input_per_mtok / 1_000_000.0
            + output_tokens as f64 * price.output_per_mtok / 1_000_000.0
            + cache_read_tokens as f64 * price.cache_read_per_mtok / 1_000_000.0;
        (cost, true)
    }

    /// Cost of `tokens` input on `model`, per token (0.0 when price unknown).
    pub fn input_price_per_token(&self, model: &str) -> f64 {
        self.lookup(model)
            .map(|p| p.input_per_mtok / 1_000_000.0)
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_worked_example_arithmetic() {
        let table = PriceTable::provider_defaults();
        let (cf, known) = table.cost("gpt-4o", 4200, 610);
        assert!(known);
        assert!((cf - 0.016600).abs() < 1e-9, "cf = {cf}");
        let (actual, known) = table.cost("gpt-4o-mini", 4200, 610);
        assert!(known);
        assert!((actual - 0.000996).abs() < 1e-9, "actual = {actual}");
        let prevented = cf - actual;
        assert!((prevented - 0.015604).abs() < 1e-9);
    }

    #[test]
    fn dated_model_ids_prefix_match() {
        let table = PriceTable::provider_defaults();
        assert!(table.lookup("gpt-4o-2024-08-06").is_some());
        assert!(table.lookup("mystery-model-x").is_none());
        let (cost, known) = table.cost("mystery-model-x", 1000, 1000);
        assert_eq!(cost, 0.0);
        assert!(!known);
    }
}
