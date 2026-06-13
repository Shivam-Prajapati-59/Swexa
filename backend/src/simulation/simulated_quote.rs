//! Simulated Quote Engine — the bridge between route candidates and the SVM.
//!
//! This module replaces the heuristic-based `QuoteEngine` with actual on-chain
//! simulation. For each candidate route, it:
//!
//! 1. Resolves the pool accounts (vaults, oracles) from cached on-chain data
//! 2. Builds the concrete DEX swap instructions for each hop
//! 3. Executes the transaction in a local lite-svm sandbox
//! 4. Reads the output token balance delta to get the exact `amount_out`
//!
//! The route with the highest simulated `amount_out` is selected as the winner.
//!
//! ## Route Splitting (Step 3)
//!
//! For large swaps, concentrating 100% of volume through one route causes
//! excessive slippage. The `optimize_with_splits` function tests split
//! allocations (e.g. 70/30, 60/40) across the top-N routes and picks
//! the combination yielding the highest total output.

use crate::simulation::account_resolver::{self, AccountCache, derive_simulated_ata};
use crate::simulation::fetcher::AccountFetcher;
use crate::simulation::instruction_builder::{
    build_swap_instruction, output_mint_for_hop, resolve_direction,
};
use crate::simulation::simulator::{QuoteSimulator, SimulationQuote};
use crate::routing::Route;
use crate::types::PoolEdge;
use anyhow::{Result, anyhow};
use serde::Serialize;
use solana_account::Account;
use solana_address::Address;
use solana_client::rpc_client::RpcClient;

use solana_keypair::Keypair;
use solana_program_option::COption;
use solana_program_pack::Pack;
use solana_signer::Signer;
use spl_token_interface::state::{Account as SplTokenAccount, AccountState};

// ── Public Types ───────────────────────────────────────────────────────────

/// Result of simulating a single route.
#[derive(Debug, Clone, Serialize)]
pub struct SimulatedRouteQuote {
    pub route_index: usize,
    pub amount_in: u64,
    pub amount_out: u64,
    pub hops: usize,
    /// `true` if the simulation succeeded, `false` if it fell back to heuristic.
    pub simulated: bool,
    /// Human-readable reason if simulation failed for this route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

/// Result of the overall best-route search (possibly with splits).
#[derive(Debug, Clone, Serialize)]
pub struct SimulatedBestQuote {
    /// The method used: "simulated" or "heuristic-fallback".
    pub quote_method: &'static str,
    /// The overall best result: either a single route or a split.
    pub best: SimulatedRouteQuote,
    /// All individual route simulations (sorted by amount_out descending).
    pub all_quotes: Vec<SimulatedRouteQuote>,
    /// If route splitting improved the result, details are here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitResult>,
}

/// Describes a volume split across multiple routes.
#[derive(Debug, Clone, Serialize)]
pub struct SplitResult {
    pub total_amount_out: u64,
    pub legs: Vec<SplitLeg>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SplitLeg {
    pub route_index: usize,
    pub amount_in: u64,
    pub amount_out: u64,
    /// Fraction of total volume allocated to this leg (0.0 – 1.0).
    pub weight: f64,
}

// ── Core Engine ────────────────────────────────────────────────────────────

/// The simulator-backed quote engine.
///
/// Lifecycle:
/// 1. Create via `SimulatedQuoteEngine::new(rpc_url)`
/// 2. Call `find_best_route` with candidate routes
/// 3. Internally fetches required accounts, loads them into the SVM, simulates
pub struct SimulatedQuoteEngine {
    rpc_url: String,
}

impl SimulatedQuoteEngine {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
        }
    }

    /// Simulates all candidate routes and returns the best one.
    ///
    /// This is the main entry point that replaces `QuoteEngine::find_best_route`.
    pub fn find_best_route(
        &self,
        routes: &[Route],
        source_mint: &str,
        target_mint: &str,
        amount_in: u64,
    ) -> Result<SimulatedBestQuote> {
        if routes.is_empty() {
            return Err(anyhow!("no candidate routes to simulate"));
        }

        // Step 1: Collect all unique pool addresses across all routes
        let all_pools: Vec<&PoolEdge> = routes
            .iter()
            .flat_map(|route| route.iter())
            .collect();

        // Step 2: Fetch on-chain accounts for all pools
        let account_cache = self.fetch_pool_accounts(&all_pools)?;

        // Step 3: Simulate each route individually
        let mut all_quotes: Vec<SimulatedRouteQuote> = routes
            .iter()
            .enumerate()
            .map(|(index, route)| {
                self.simulate_single_route(
                    index,
                    route,
                    source_mint,
                    target_mint,
                    amount_in,
                    &account_cache,
                )
            })
            .collect();

        // Sort by amount_out descending and filter out zero-output routes
        all_quotes.sort_by(|a, b| b.amount_out.cmp(&a.amount_out));
        all_quotes.retain(|q| q.amount_out > 0);

        let best_single = all_quotes
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("all route simulations failed"))?;

        // Step 4: Try route splitting across top routes
        let split = if all_quotes.len() >= 2 && amount_in >= 10_000 {
            self.optimize_with_splits(
                routes,
                source_mint,
                target_mint,
                amount_in,
                &account_cache,
                &all_quotes,
            )
        } else {
            None
        };

        // Determine the overall best (single vs split)
        let (best, quote_method) = match &split {
            Some(split_result) if split_result.total_amount_out > best_single.amount_out => {
                let combined = SimulatedRouteQuote {
                    route_index: split_result.legs[0].route_index,
                    amount_in,
                    amount_out: split_result.total_amount_out,
                    hops: 0, // split is multi-route
                    simulated: true,
                    fallback_reason: None,
                };
                (combined, "simulated-split")
            }
            _ => {
                let method = if best_single.simulated {
                    "simulated"
                } else {
                    "heuristic-fallback"
                };
                (best_single, method)
            }
        };

        Ok(SimulatedBestQuote {
            quote_method,
            best,
            all_quotes,
            split,
        })
    }

    /// Simulates a single route end-to-end in the SVM.
    fn simulate_single_route(
        &self,
        route_index: usize,
        route: &Route,
        source_mint: &str,
        _target_mint: &str,
        amount_in: u64,
        account_cache: &AccountCache,
    ) -> SimulatedRouteQuote {
        match self.try_simulate_route(route, source_mint, amount_in, account_cache) {
            Ok(quote) => SimulatedRouteQuote {
                route_index,
                amount_in,
                amount_out: quote.amount_out,
                hops: route.len(),
                simulated: true,
                fallback_reason: None,
            },
            Err(err) => {
                // Fall back to the heuristic estimator for this route
                let heuristic_out = crate::engine::quote::QuoteEngine::quote_route(route, amount_in)
                    .map(|q| q.estimated_amount_out)
                    .unwrap_or(0);

                SimulatedRouteQuote {
                    route_index,
                    amount_in,
                    amount_out: heuristic_out,
                    hops: route.len(),
                    simulated: false,
                    fallback_reason: Some(format!("simulation failed: {err}")),
                }
            }
        }
    }

    /// Attempts to simulate a route. Returns `Err` if any step fails.
    fn try_simulate_route(
        &self,
        route: &Route,
        source_mint: &str,
        amount_in: u64,
        account_cache: &AccountCache,
    ) -> Result<SimulationQuote> {
        let payer = Keypair::new();
        let payer_pubkey = payer.pubkey();
        let mut simulator = QuoteSimulator::new();

        // Give the payer enough SOL for transaction fees
        simulator.airdrop(&payer_pubkey, 10_000_000_000)?;

        // Load all cached accounts into the SVM
        for (address, account) in account_cache {
            simulator.load_accounts(vec![(*address, account.clone())])?;
        }

        // Build the full instruction sequence for all hops
        let mut instructions = Vec::new();
        let mut current_mint = source_mint.to_string();
        let mut current_amount = amount_in;

        // Determine the final output mint and create the target token account
        let mut final_mint = source_mint.to_string();
        for pool in route.iter() {
            final_mint = output_mint_for_hop(pool, &final_mint)?.to_string();
        }
        let target_ata = derive_simulated_ata(&payer_pubkey, &final_mint);

        // Create and load simulated token accounts into the SVM
        // Source token account (loaded with amount_in)
        let source_ata = derive_simulated_ata(&payer_pubkey, source_mint);
        let source_token_mint = parse_mint_address(source_mint)?;
        let source_token_acct = create_token_account(amount_in, source_token_mint, payer_pubkey);
        simulator.load_accounts(vec![(source_ata, source_token_acct)])?;

        // Destination token account (starts at 0)
        let target_token_mint = parse_mint_address(&final_mint)?;
        let target_token_acct = create_token_account(0, target_token_mint, payer_pubkey);
        simulator.load_accounts(vec![(target_ata, target_token_acct)])?;

        // Create intermediate token accounts for multi-hop routes
        let mut intermediate_atas = Vec::new();
        let mut temp_mint = source_mint.to_string();
        for (i, pool) in route.iter().enumerate() {
            let next_mint = output_mint_for_hop(pool, &temp_mint)?.to_string();

            if i < route.len() - 1 {
                // Intermediate hop — create an intermediate ATA
                let inter_ata = derive_simulated_ata(&payer_pubkey, &next_mint);
                let inter_mint = parse_mint_address(&next_mint)?;
                let inter_acct = create_token_account(0, inter_mint, payer_pubkey);
                simulator.load_accounts(vec![(inter_ata, inter_acct)])?;
                intermediate_atas.push((next_mint.clone(), inter_ata));
            }

            temp_mint = next_mint;
        }

        // Build instructions for each hop
        let mut hop_source_ata = source_ata;
        let mut hop_mint = source_mint.to_string();

        for (i, pool) in route.iter().enumerate() {
            let direction = resolve_direction(pool, &hop_mint)?;
            let next_mint = output_mint_for_hop(pool, &hop_mint)?.to_string();

            let hop_dest_ata = if i == route.len() - 1 {
                target_ata
            } else {
                intermediate_atas[i].1
            };

            let swap_accounts = account_resolver::resolve_swap_accounts(
                pool,
                &hop_mint,
                &payer_pubkey,
                &hop_source_ata,
                &hop_dest_ata,
                account_cache,
            )?;

            let hop_instructions = build_swap_instruction(
                pool,
                direction,
                current_amount,
                0, // minimum_amount_out = 0 for simulation (we want to see actual output)
                &swap_accounts,
            )?;

            instructions.extend(hop_instructions);

            // For subsequent hops, we don't know the exact intermediate amount
            // until the simulation runs. We use 0 as a placeholder since the
            // actual on-chain program will operate on whatever balance is present.
            current_amount = 0;
            hop_source_ata = hop_dest_ata;
            hop_mint = next_mint;
        }

        // Execute the simulation
        simulator.simulate_transaction(&instructions, &payer, &target_ata)
    }

    /// Fetches on-chain accounts for all pools in the candidate routes.
    fn fetch_pool_accounts(&self, pools: &[&PoolEdge]) -> Result<AccountCache> {
        let rpc_client = RpcClient::new(&self.rpc_url);
        let mut all_required_keys = Vec::new();

        for pool in pools {
            match account_resolver::required_accounts_for_pool(pool) {
                Ok(keys) => all_required_keys.extend(keys),
                Err(err) => {
                    eprintln!(
                        "[SimQuote] skipping pool {} — failed to resolve required accounts: {}",
                        pool.address, err
                    );
                }
            }
        }

        // Dedup
        all_required_keys.sort_by_key(|k| k.to_bytes());
        all_required_keys.dedup_by_key(|k| k.to_bytes());

        if all_required_keys.is_empty() {
            return Ok(AccountCache::new());
        }

        // Convert Address → Pubkey for the RPC client
        let pubkeys: Vec<solana_sdk::pubkey::Pubkey> = all_required_keys
            .iter()
            .map(|addr| solana_sdk::pubkey::Pubkey::new_from_array(addr.to_bytes()))
            .collect();

        let fetched = AccountFetcher::fetch_accounts(&rpc_client, &pubkeys)?;

        let mut cache = AccountCache::with_capacity(fetched.len());
        for (address, account) in fetched {
            cache.insert(address, account);
        }

        Ok(cache)
    }

    // ── Route Splitting (Step 3) ───────────────────────────────────────────

    /// Tests various volume splits across the top-N routes to see if splitting
    /// yields more output than a single route.
    ///
    /// We test splits like: 90/10, 80/20, 70/30, 60/40, 50/50 across the
    /// top 2-3 routes and pick the split that maximizes total output.
    fn optimize_with_splits(
        &self,
        routes: &[Route],
        source_mint: &str,
        _target_mint: &str,
        amount_in: u64,
        account_cache: &AccountCache,
        ranked_quotes: &[SimulatedRouteQuote],
    ) -> Option<SplitResult> {
        // Only consider the top 3 routes for splitting
        let top_n = ranked_quotes.iter().take(3).collect::<Vec<_>>();
        if top_n.len() < 2 {
            return None;
        }

        // Define split ratios to test (2-way splits)
        let split_ratios: &[(f64, f64)] = &[
            (0.90, 0.10),
            (0.80, 0.20),
            (0.70, 0.30),
            (0.60, 0.40),
            (0.50, 0.50),
        ];

        let mut best_split: Option<SplitResult> = None;
        let best_single_out = ranked_quotes.first().map(|q| q.amount_out).unwrap_or(0);

        // Test 2-way splits between top 2 routes
        for &(w1, w2) in split_ratios {
            let idx1 = top_n[0].route_index;
            let idx2 = top_n[1].route_index;

            let amount1 = (amount_in as f64 * w1) as u64;
            let amount2 = amount_in.saturating_sub(amount1); // ensure no rounding loss

            let out1 = self
                .simulate_split_leg(&routes[idx1], source_mint, amount1, account_cache)
                .unwrap_or(0);
            let out2 = self
                .simulate_split_leg(&routes[idx2], source_mint, amount2, account_cache)
                .unwrap_or(0);

            // NOTE: When routes share common pools, simulating each leg independently
            // can overestimate total output because the second leg doesn't see the
            // state impact of the first leg's trade. A sequential simulation that
            // updates account_cache between legs would be more accurate, but adds
            // significant complexity. For now we accept this optimistic bias — the
            // on-chain execution with slippage protection will catch any overestimate.
            let total_out = out1 + out2;
            if total_out > best_single_out {
                let candidate = SplitResult {
                    total_amount_out: total_out,
                    legs: vec![
                        SplitLeg {
                            route_index: idx1,
                            amount_in: amount1,
                            amount_out: out1,
                            weight: w1,
                        },
                        SplitLeg {
                            route_index: idx2,
                            amount_in: amount2,
                            amount_out: out2,
                            weight: w2,
                        },
                    ],
                };

                if best_split
                    .as_ref()
                    .map_or(true, |prev| total_out > prev.total_amount_out)
                {
                    best_split = Some(candidate);
                }
            }
        }

        // If we have 3+ routes, also test 3-way splits
        if top_n.len() >= 3 {
            let three_way_splits: &[(f64, f64, f64)] = &[
                (0.50, 0.30, 0.20),
                (0.40, 0.35, 0.25),
                (0.60, 0.25, 0.15),
            ];

            for &(w1, w2, w3) in three_way_splits {
                let idx1 = top_n[0].route_index;
                let idx2 = top_n[1].route_index;
                let idx3 = top_n[2].route_index;

                let amount1 = (amount_in as f64 * w1) as u64;
                let amount2 = (amount_in as f64 * w2) as u64;
                let amount3 = amount_in.saturating_sub(amount1).saturating_sub(amount2);

                let out1 = self
                    .simulate_split_leg(&routes[idx1], source_mint, amount1, account_cache)
                    .unwrap_or(0);
                let out2 = self
                    .simulate_split_leg(&routes[idx2], source_mint, amount2, account_cache)
                    .unwrap_or(0);
                let out3 = self
                    .simulate_split_leg(&routes[idx3], source_mint, amount3, account_cache)
                    .unwrap_or(0);

                let total_out = out1 + out2 + out3;
                if total_out > best_single_out {
                    let candidate = SplitResult {
                        total_amount_out: total_out,
                        legs: vec![
                            SplitLeg {
                                route_index: idx1,
                                amount_in: amount1,
                                amount_out: out1,
                                weight: w1,
                            },
                            SplitLeg {
                                route_index: idx2,
                                amount_in: amount2,
                                amount_out: out2,
                                weight: w2,
                            },
                            SplitLeg {
                                route_index: idx3,
                                amount_in: amount3,
                                amount_out: out3,
                                weight: w3,
                            },
                        ],
                    };

                    if best_split
                        .as_ref()
                        .map_or(true, |prev| total_out > prev.total_amount_out)
                    {
                        best_split = Some(candidate);
                    }
                }
            }
        }

        best_split
    }

    /// Simulates a split leg (a single route with a fraction of the total volume).
    fn simulate_split_leg(
        &self,
        route: &Route,
        source_mint: &str,
        amount_in: u64,
        account_cache: &AccountCache,
    ) -> Result<u64> {
        if amount_in == 0 {
            return Ok(0);
        }

        match self.try_simulate_route(route, source_mint, amount_in, account_cache) {
            Ok(quote) => Ok(quote.amount_out),
            Err(err) => {
                // Fall back to heuristic for the split leg
                crate::engine::quote::QuoteEngine::quote_route(route, amount_in)
                    .map(|q| q.estimated_amount_out)
                    .ok_or_else(|| anyhow!("heuristic fallback also failed: {err}"))
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Creates a simulated SPL token account with the given balance.
fn create_token_account(amount: u64, mint: Address, owner: Address) -> Account {
    let token_account = SplTokenAccount {
        mint,
        owner,
        amount,
        delegate: COption::None,
        state: AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: 0,
        close_authority: COption::None,
    };
    let mut data = vec![0u8; SplTokenAccount::LEN];
    SplTokenAccount::pack(token_account, &mut data).expect("SPL token account pack failed");

    // SPL Token program ID: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
    // Raw bytes of the well-known SPL Token program address.
    const TOKEN_PROGRAM_BYTES: [u8; 32] = [
        6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172,
        28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
    ];

    Account {
        lamports: 1_000_000,
        data,
        owner: Address::from(TOKEN_PROGRAM_BYTES),
        executable: false,
        rent_epoch: 0,
    }
}

fn parse_mint_address(mint_str: &str) -> Result<Address> {
    // For simulation purposes, if the mint string is not a valid base58
    // address (e.g. test fixtures), we derive a deterministic address from it.
    let decoded = bs58_decode_opt(mint_str);
    match decoded {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(Address::from(arr))
        }
        _ => {
            // Derive deterministic 32-byte address from mint string using
            // multiple hash rounds to fill the entire address space.
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            let mut bytes = [0u8; 32];

            mint_str.hash(&mut hasher);
            let h1 = hasher.finish();
            bytes[0..8].copy_from_slice(&h1.to_le_bytes());

            mint_str.as_bytes().hash(&mut hasher);
            let h2 = hasher.finish();
            bytes[8..16].copy_from_slice(&h2.to_le_bytes());

            mint_str.len().hash(&mut hasher);
            let h3 = hasher.finish();
            bytes[16..24].copy_from_slice(&h3.to_le_bytes());

            "mint_salt".hash(&mut hasher);
            let h4 = hasher.finish();
            bytes[24..32].copy_from_slice(&h4.to_le_bytes());

            Ok(Address::from(bytes))
        }
    }
}

fn bs58_decode_opt(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

    let mut result: Vec<u8> = Vec::new();
    let mut leading_zeros = 0;

    for ch in input.bytes() {
        if ch == b'1' && result.is_empty() {
            leading_zeros += 1;
            continue;
        }

        let digit = ALPHABET.iter().position(|&c| c == ch)?;
        let mut carry = digit;
        for byte in result.iter_mut().rev() {
            let v = (*byte as usize) * 58 + carry;
            *byte = (v % 256) as u8;
            carry = v / 256;
        }
        while carry > 0 {
            result.insert(0, (carry % 256) as u8);
            carry /= 256;
        }
    }

    let mut output = vec![0u8; leading_zeros];
    output.extend(result);
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DexProtocol, PoolType, TokenMint};

    fn token(mint: &str) -> TokenMint {
        TokenMint {
            mint: mint.to_string(),
            symbol: mint.to_string(),
            decimals: 9,
        }
    }

    fn pool(address: &str, a: &str, b: &str, tvl: f64) -> PoolEdge {
        PoolEdge {
            address: address.to_string(),
            dex: DexProtocol::Raydium,
            token_a: token(a),
            token_b: token(b),
            fee_rate: 0.003,
            tvl,
            pool_type: PoolType::Amm,
        }
    }

    #[test]
    fn create_token_account_has_correct_balance() {
        let mint = Address::from([1u8; 32]);
        let owner = Address::from([2u8; 32]);
        let acct = create_token_account(42_000, mint, owner);

        let unpacked = SplTokenAccount::unpack(&acct.data).unwrap();
        assert_eq!(unpacked.amount, 42_000);
        assert_eq!(unpacked.mint, mint);
        assert_eq!(unpacked.owner, owner);
    }

    #[test]
    fn parse_mint_address_handles_non_base58() {
        // Should not panic — derives a deterministic address
        let addr = parse_mint_address("SOL_TEST_MINT").unwrap();
        let addr2 = parse_mint_address("SOL_TEST_MINT").unwrap();
        assert_eq!(addr, addr2);
    }

    #[test]
    fn simulated_engine_errors_on_empty_routes() {
        let engine = SimulatedQuoteEngine::new("http://localhost:8899");
        let routes: Vec<Route> = vec![];
        let result = engine.find_best_route(&routes, "A", "B", 1000);
        assert!(result.is_err());
    }
}
