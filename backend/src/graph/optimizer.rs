// Routing optimizer
// Uses pool-type-specific exact math to rank swap routes by estimated output.

use crate::graph::builder::RouteCandidate;
use crate::models::pool::{Pool, PoolId};
use std::collections::HashMap;

/// Ranks candidate routes by simulating each hop with pool-type-specific exact math.
///
/// - CPMM pools use the constant-product invariant `dy = y * dx' / (x + dx')`
/// - StableSwap pools use Newton's method on the Curve invariant
/// - CLMM/DLMM pools fall back to linear spot-price (no tick-crossing simulation)
///
/// The simulation accounts for both protocol fees and price impact at every hop.
pub fn rank_candidates(
    candidates: Vec<RouteCandidate>,
    amount_in: u64,
    pools: &[Pool],
    top_k: usize,
) -> Vec<(RouteCandidate, u64)> {
    let pool_map: HashMap<PoolId, &Pool> = pools.iter().map(|p| (p.pool_id(), p)).collect();
    let mut ranked = Vec::new();

    for candidate in candidates {
        let mut current_amount = amount_in as f64;
        let mut valid = true;

        for step in &candidate.steps {
            let pool = match pool_map.get(&step.pool_id) {
                Some(p) => p,
                None => {
                    valid = false;
                    break;
                }
            };

            let estimated_out = pool
                .simulate_swap(&step.input_mint, current_amount as u64)
                .map(|v| v as f64)
                .unwrap_or(0.0);

            current_amount = estimated_out;

            if !current_amount.is_finite() || current_amount <= 0.0 {
                valid = false;
                break;
            }
        }

        if valid {
            ranked.push((candidate, current_amount as u64));
        }
    }

    ranked.sort_by_key(|b| std::cmp::Reverse(b.1));
    ranked.truncate(top_k);
    ranked
}
