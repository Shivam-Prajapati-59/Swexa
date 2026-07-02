use crate::graph::builder::GraphBuilder;
use crate::graph::optimizer::{self, SimulatedRoute};
use crate::models::pool::{Pool, PubkeyBytes};
use crate::services::hydration_service::hydrate_candidate_pools;
use solana_client::nonblocking::rpc_client::RpcClient;

const HEURISTIC_CANDIDATE_LIMIT: usize = 50;

pub async fn rank_best_routes(
    builder: &GraphBuilder,
    pools: &[Pool],
    input_mint: &PubkeyBytes,
    output_mint: &PubkeyBytes,
    amount: u128,
    final_limit: usize,
    rpc: Option<&RpcClient>,
) -> Vec<SimulatedRoute> {
    let raw_routes = builder
        .find_all_routes(input_mint, output_mint, 4, 20_000)
        .unwrap_or_default();

    let cheap_ranked = optimizer::rank_candidates(
        raw_routes.clone(),
        amount,
        pools,
        HEURISTIC_CANDIDATE_LIMIT.max(final_limit),
    );

    if cheap_ranked.is_empty() {
        return cheap_ranked;
    }

    let Some(rpc) = rpc else {
        return cheap_ranked.into_iter().take(final_limit).collect();
    };

    let candidates_to_hydrate: Vec<_> = cheap_ranked
        .iter()
        .map(|route| route.candidate.clone())
        .collect();
    let hydrated_pools = hydrate_candidate_pools(pools, &candidates_to_hydrate, rpc).await;

    let final_ranked =
        optimizer::rank_candidates(candidates_to_hydrate, amount, &hydrated_pools, final_limit);

    if final_ranked.is_empty() {
        cheap_ranked.into_iter().take(final_limit).collect()
    } else {
        final_ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::builder::GraphBuilder;
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
}
