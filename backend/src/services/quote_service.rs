use crate::graph::builder::{GraphBuilder, RouteCandidate};
use crate::graph::optimizer::{self, SimulatedRoute};
use crate::models::pool::{Pool, PubkeyBytes};
use crate::services::hydration_service::hydrate_candidate_pools;
use solana_client::nonblocking::rpc_client::RpcClient;
use std::collections::{BTreeSet, HashMap};

const ROUTE_DISCOVERY_LIMIT: usize = 20_000;
const MAX_HOPS: usize = 4;

pub struct QuotePlan {
    candidates: Vec<RouteCandidate>,
    pools: Vec<Pool>,
}

pub async fn rank_best_routes(
    builder: &GraphBuilder,
    pools: &[Pool],
    input_mint: &PubkeyBytes,
    output_mint: &PubkeyBytes,
    amount: u128,
    final_limit: usize,
    rpc: Option<&RpcClient>,
) -> Vec<SimulatedRoute> {
    let Some(plan) = plan_best_routes(builder, pools, input_mint, output_mint, final_limit) else {
        return Vec::new();
    };

    execute_quote_plan(plan, amount, final_limit, rpc).await
}

pub fn plan_best_routes(
    builder: &GraphBuilder,
    pools: &[Pool],
    input_mint: &PubkeyBytes,
    output_mint: &PubkeyBytes,
    final_limit: usize,
) -> Option<QuotePlan> {
    let candidates = builder
        .find_all_routes(input_mint, output_mint, MAX_HOPS, ROUTE_DISCOVERY_LIMIT)
        .unwrap_or_default();

    if candidates.is_empty() {
        return None;
    }

    let selected_candidates = select_heuristic_candidates(candidates, final_limit);
    let selected_pools = candidate_pool_subset(pools, &selected_candidates);

    Some(QuotePlan {
        candidates: selected_candidates,
        pools: selected_pools,
    })
}

pub async fn execute_quote_plan(
    plan: QuotePlan,
    amount: u128,
    final_limit: usize,
    rpc: Option<&RpcClient>,
) -> Vec<SimulatedRoute> {
    let Some(rpc) = rpc else {
        return optimizer::rank_candidates(plan.candidates, amount, &plan.pools, final_limit);
    };

    let hydrated_pools = hydrate_candidate_pools(&plan.pools, &plan.candidates, rpc).await;

    optimizer::rank_candidates(plan.candidates, amount, &hydrated_pools, final_limit)
}

fn select_heuristic_candidates(
    mut candidates: Vec<RouteCandidate>,
    final_limit: usize,
) -> Vec<RouteCandidate> {
    candidates.sort_by(|a, b| {
        a.total_cost
            .partial_cmp(&b.total_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(final_limit);
    candidates
}

fn candidate_pool_subset(pools: &[Pool], candidates: &[RouteCandidate]) -> Vec<Pool> {
    let target_ids: BTreeSet<_> = candidates
        .iter()
        .flat_map(|route| route.steps.iter().map(|step| step.pool_id))
        .collect();
    let pool_by_id: HashMap<_, _> = pools.iter().map(|pool| (pool.pool_id(), pool)).collect();

    target_ids
        .into_iter()
        .filter_map(|pool_id| pool_by_id.get(&pool_id).map(|pool| (*pool).clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::builder::{GraphBuilder, RouteCandidate, RouteStep};
    use crate::models::pool::{
        CpmmState, DexProtocol, PoolData, PoolMetadata, PoolStatus, PoolToken, PoolType,
    };

    fn mint(byte: u8) -> PubkeyBytes {
        PubkeyBytes([byte; 32])
    }

    #[tokio::test]
    async fn rank_best_routes_uses_cached_metadata_without_rpc() {
        let token_a = mint(1);
        let token_b = mint(2);
        let pools = vec![Pool {
            metadata: PoolMetadata {
                id: 1,
                pubkey: mint(9),
                protocol: DexProtocol::Raydium,
                pool_type: PoolType::Cpmm,
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
                reserve_b: 1_000_000,
            }),
            fee_rate: 0,
            tvl: Some(100_000.0),
            last_updated_slot: None,
        }];

        let mut builder = GraphBuilder::new();
        builder.build_from_pools(&pools, 0.0);

        let ranked = rank_best_routes(&builder, &pools, &token_a, &token_b, 1_000, 10, None).await;

        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].estimated_amount_out > 0);
    }

    #[test]
    fn plan_best_routes_clones_only_selected_route_pools() {
        let token_a = mint(1);
        let token_b = mint(2);
        let token_c = mint(3);
        let unused_token = mint(4);
        let pools = vec![
            test_pool(1, token_a, token_b),
            test_pool(2, token_a, token_c),
            test_pool(3, unused_token, token_b),
        ];

        let mut builder = GraphBuilder::new();
        builder.build_from_pools(&pools, 0.0);

        let plan = plan_best_routes(&builder, &pools, &token_a, &token_b, 1).unwrap();
        let planned_ids: Vec<u32> = plan.pools.iter().map(|pool| pool.pool_id()).collect();

        assert_eq!(planned_ids, vec![1]);
    }

    #[test]
    fn select_heuristic_candidates_uses_route_cost_before_simulation() {
        let token_a = mint(1);
        let token_b = mint(2);

        let selected = select_heuristic_candidates(
            vec![
                candidate(1, token_a, token_b, 10.0),
                candidate(2, token_a, token_b, 1.0),
                candidate(3, token_a, token_b, 5.0),
            ],
            2,
        );

        let selected_ids: Vec<u32> = selected
            .iter()
            .map(|route| route.steps[0].pool_id)
            .collect();

        assert_eq!(selected_ids, vec![2, 3]);
    }

    fn candidate(
        pool_id: u32,
        input_mint: PubkeyBytes,
        output_mint: PubkeyBytes,
        total_cost: f64,
    ) -> RouteCandidate {
        RouteCandidate {
            steps: vec![RouteStep {
                pool_id,
                input_mint,
                output_mint,
            }],
            total_cost,
        }
    }

    fn test_pool(id: u32, token_a: PubkeyBytes, token_b: PubkeyBytes) -> Pool {
        Pool {
            metadata: PoolMetadata {
                id,
                pubkey: mint(id as u8 + 10),
                protocol: DexProtocol::Raydium,
                pool_type: PoolType::Cpmm,
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
                reserve_b: 1_000_000,
            }),
            fee_rate: 0,
            tvl: Some(100_000.0 - id as f64),
            last_updated_slot: None,
        }
    }
}
