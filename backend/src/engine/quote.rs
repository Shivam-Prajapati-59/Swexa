use crate::routing::Route;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RouteQuote {
    pub amount_in: u64,
    pub estimated_amount_out: u64,
    pub price_impact_bps: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BestRouteQuote {
    pub best_route_index: usize,
    pub quote: RouteQuote,
}

/// Ranks candidate routes with the metadata currently available in `PoolEdge`.
///
/// This is the Phase 1 quote boundary: when DEX instruction builders are wired
/// in, this estimator can be replaced by the SVM-backed simulator without
/// changing the API or graph pathfinding flow.
pub struct QuoteEngine;

impl QuoteEngine {
    pub fn quote_route(route: &Route, amount_in: u64) -> Option<RouteQuote> {
        if route.is_empty() {
            return None;
        }

        let mut amount = amount_in as f64;
        let mut total_price_impact_bps: f64 = 0.0;

        for pool in route {
            if amount <= 0.0
                || !pool.fee_rate.is_finite()
                || !pool.tvl.is_finite()
                || pool.tvl <= 0.0
            {
                return None;
            }

            let amount_after_fee = amount * (1.0 - pool.fee_rate.clamp(0.0, 1.0));
            let liquidity = pool.tvl.max(1.0);
            let slippage_factor = liquidity / (liquidity + amount_after_fee);
            let price_impact_bps = (1.0 - slippage_factor) * 10_000.0;

            amount = amount_after_fee * slippage_factor;
            total_price_impact_bps += price_impact_bps;
        }

        Some(RouteQuote {
            amount_in,
            estimated_amount_out: amount.floor().max(0.0) as u64,
            price_impact_bps: total_price_impact_bps.round().max(0.0) as u64,
        })
    }

    pub fn find_best_route(routes: &[Route], amount_in: u64) -> Option<BestRouteQuote> {
        routes
            .iter()
            .enumerate()
            .filter_map(|(index, route)| {
                Self::quote_route(route, amount_in).map(|quote| BestRouteQuote {
                    best_route_index: index,
                    quote,
                })
            })
            .max_by_key(|candidate| candidate.quote.estimated_amount_out)
    }
}

#[cfg(test)]
mod tests {
    use super::QuoteEngine;
    use crate::types::{DexProtocol, PoolEdge, PoolType, TokenMint};

    fn token(mint: &str) -> TokenMint {
        TokenMint {
            mint: mint.to_string(),
            symbol: mint.to_string(),
            decimals: 9,
        }
    }

    fn pool(address: &str, tvl: f64, fee_rate: f64) -> PoolEdge {
        PoolEdge {
            address: address.to_string(),
            dex: DexProtocol::Raydium,
            token_a: token("A"),
            token_b: token("B"),
            fee_rate,
            tvl,
            pool_type: PoolType::Amm,
        }
    }

    #[test]
    fn best_route_prefers_highest_estimated_output() {
        let routes = vec![
            vec![pool("low-liquidity", 1_000.0, 0.003)],
            vec![pool("high-liquidity", 100_000.0, 0.003)],
        ];

        let best = QuoteEngine::find_best_route(&routes, 1_000).unwrap();

        assert_eq!(best.best_route_index, 1);
        assert!(best.quote.estimated_amount_out > 900);
    }
}
