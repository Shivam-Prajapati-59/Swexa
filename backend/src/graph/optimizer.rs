// Routing optimizer
// Uses pool-type-specific exact math to rank swap routes by estimated output.

use crate::graph::builder::RouteCandidate;
use crate::models::graph::{SimulatedHop, TokenAmount};
use crate::models::pool::{Pool, PoolId, PubkeyBytes};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SimulatedRoute {
    pub candidate: RouteCandidate,
    pub estimated_amount_out: u128,
    pub total_fees: Vec<TokenAmount>,
    pub max_price_impact_pct: f64,
    pub has_approximate_hops: bool,
    pub hops: Vec<SimulatedHop>,
}

/// Ranks candidate routes by simulating each hop with pool-type-specific exact math.
///
/// - CPMM pools use the constant-product invariant `dy = y * dx' / (x + dx')`
/// - StableSwap pools use Newton's method on the Curve invariant
/// - CLMM pools use a Q64.64 virtual reserve approximation
/// - DLMM pools use a linear active-bin spot approximation
///
/// The simulation accounts for both protocol fees and price impact at every hop.
pub fn rank_candidates(
    candidates: Vec<RouteCandidate>,
    amount_in: u128,
    pools: &[Pool],
    top_k: usize,
) -> Vec<SimulatedRoute> {
    let pool_map: HashMap<PoolId, &Pool> = pools.iter().map(|p| (p.pool_id(), p)).collect();
    let mut ranked = Vec::new();

    for candidate in candidates {
        let mut current_amount = amount_in;
        let mut total_fees: HashMap<PubkeyBytes, u128> = HashMap::new();
        let mut max_price_impact_pct = 0.0f64;
        let mut has_approximate_hops = false;
        let mut simulated_hops = Vec::with_capacity(candidate.steps.len());
        let mut valid = true;

        for step in &candidate.steps {
            let pool = match pool_map.get(&step.pool_id) {
                Some(p) => p,
                None => {
                    valid = false;
                    break;
                }
            };

            let amount_before = current_amount;
            let result = match pool.simulate_swap(&step.input_mint, current_amount) {
                Ok(result) => result,
                Err(_) => {
                    valid = false;
                    break;
                }
            };

            current_amount = result.amount_out;
            let fee_total = total_fees.entry(step.input_mint).or_insert(0);
            *fee_total = match fee_total.checked_add(result.fee_amount) {
                Some(fee_total) => fee_total,
                None => {
                    valid = false;
                    break;
                }
            };
            max_price_impact_pct = max_price_impact_pct.max(result.price_impact_pct);
            has_approximate_hops |= result.is_approximate;

            simulated_hops.push(SimulatedHop {
                pool_id: step.pool_id,
                input_mint: step.input_mint,
                output_mint: step.output_mint,
                amount_in: amount_before,
                amount_out: result.amount_out,
                fee_amount: result.fee_amount,
                price_impact_pct: result.price_impact_pct,
                is_approximate: result.is_approximate,
            });
        }

        if valid {
            let mut total_fees: Vec<TokenAmount> = total_fees
                .into_iter()
                .map(|(mint, amount)| TokenAmount { mint, amount })
                .collect();
            total_fees.sort_by(|a, b| a.mint.cmp(&b.mint));

            ranked.push(SimulatedRoute {
                candidate,
                estimated_amount_out: current_amount,
                total_fees,
                max_price_impact_pct,
                has_approximate_hops,
                hops: simulated_hops,
            });
        }
    }

    ranked.sort_by(|a, b| b.estimated_amount_out.cmp(&a.estimated_amount_out));
    ranked.truncate(top_k);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::builder::{RouteCandidate, RouteStep};
    use crate::models::pool::{
        CpmmState, DexProtocol, PoolData, PoolMetadata, PoolStatus, PoolToken, PoolType,
        PubkeyBytes,
    };

    fn mint(byte: u8) -> PubkeyBytes {
        PubkeyBytes([byte; 32])
    }

    fn cpmm_pool(
        id: PoolId,
        token_a: PubkeyBytes,
        token_b: PubkeyBytes,
        reserve_b: u128,
        fee_rate: u32,
    ) -> Pool {
        Pool {
            metadata: PoolMetadata {
                id,
                pubkey: mint(id as u8 + 10),
                protocol: DexProtocol::Raydium,
                pool_type: PoolType::AMM,
                status: PoolStatus::Active,
                token_a: PoolToken {
                    mint: token_a,
                    name: "A".to_string(),
                    symbol: "A".to_string(),
                    decimals: 6,
                    vault: None,
                },
                token_b: PoolToken {
                    mint: token_b,
                    name: "B".to_string(),
                    symbol: "B".to_string(),
                    decimals: 6,
                    vault: None,
                },
            },
            data: PoolData::Cpmm(CpmmState {
                reserve_a: 1_000_000,
                reserve_b,
            }),
            fee_rate,
            tvl: Some(10_000.0),
            last_updated_slot: None,
        }
    }

    #[test]
    fn rank_candidates_preserves_simulated_hops_and_sorts_by_output() {
        let token_a = mint(1);
        let token_b = mint(2);
        let pools = vec![
            cpmm_pool(1, token_a, token_b, 1_000_000, 0),
            cpmm_pool(2, token_a, token_b, 2_000_000, 0),
        ];
        let candidates = vec![
            RouteCandidate {
                steps: vec![RouteStep {
                    pool_id: 1,
                    input_mint: token_a,
                    output_mint: token_b,
                }],
                total_cost: 1.0,
            },
            RouteCandidate {
                steps: vec![RouteStep {
                    pool_id: 2,
                    input_mint: token_a,
                    output_mint: token_b,
                }],
                total_cost: 2.0,
            },
        ];

        let ranked = rank_candidates(candidates, 10_000, &pools, 2);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].candidate.steps[0].pool_id, 2);
        assert_eq!(ranked[0].hops.len(), 1);
        assert_eq!(ranked[0].hops[0].amount_in, 10_000);
        assert_eq!(ranked[0].hops[0].amount_out, ranked[0].estimated_amount_out);
        assert!(ranked[0].estimated_amount_out > ranked[1].estimated_amount_out);
    }

    #[test]
    fn rank_candidates_aggregates_fees_by_input_mint() {
        let token_a = mint(1);
        let token_b = mint(2);
        let token_c = mint(3);
        let pools = vec![
            cpmm_pool(1, token_a, token_b, 1_000_000, 10_000),
            cpmm_pool(2, token_b, token_c, 1_000_000, 20_000),
        ];
        let candidates = vec![RouteCandidate {
            steps: vec![
                RouteStep {
                    pool_id: 1,
                    input_mint: token_a,
                    output_mint: token_b,
                },
                RouteStep {
                    pool_id: 2,
                    input_mint: token_b,
                    output_mint: token_c,
                },
            ],
            total_cost: 1.0,
        }];

        let ranked = rank_candidates(candidates, 100_000, &pools, 1);

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].total_fees.len(), 2);
        assert!(
            ranked[0]
                .total_fees
                .iter()
                .any(|fee| fee.mint == token_a && fee.amount == 1_000)
        );
        assert!(
            ranked[0]
                .total_fees
                .iter()
                .any(|fee| fee.mint == token_b && fee.amount > 0)
        );
    }

    #[test]
    fn rank_candidates_skips_routes_that_fail_simulation() {
        let token_a = mint(1);
        let token_b = mint(2);
        let pools = vec![cpmm_pool(1, token_a, token_b, 1_000_000, 1_000_000)];
        let candidates = vec![RouteCandidate {
            steps: vec![RouteStep {
                pool_id: 1,
                input_mint: token_a,
                output_mint: token_b,
            }],
            total_cost: 1.0,
        }];

        assert!(rank_candidates(candidates, 100_000, &pools, 1).is_empty());
    }
}
