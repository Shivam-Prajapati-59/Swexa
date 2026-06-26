# Swexa — Solana DEX Aggregator

A high-performance Rust-based DEX routing engine for Solana. Swexa discovers liquidity pools across multiple DEXes (Raydium, Meteora, Whirlpool), builds a weighted directed multigraph, and finds the most efficient swap routes between any two tokens.

## Architecture

```mermaid
flowchart TB
    subgraph Adapters[Phase 1 - Data Ingestion]
        RAY[Raydium Adapter]
        MET[Meteora Adapter]
        WHP[Whirlpool Adapter]
    end

    subgraph Cache[Phase 2 - Pool Cache]
        POOLS[AppState::pools]
    end

    subgraph Pruning[Phase 3 - Pruning Pipeline]
        F1[Status == Active?]
        F2[TVL >= 1000]
        F3[Zero-mint or self-ref?]
        F4[Top-5 per pair by TVL]
        F5[Dedup edges]
    end

    subgraph Graph[Phase 4 - Weighted Multigraph]
        direction LR
        SOL((SOL))
        USDC((USDC))
        RAY_T((RAY))
        ZEC((ZEC))

        SOL -- fee:25bp tvl:2.1M --> USDC
        SOL -- fee:30bp tvl:800K --> RAY_T
        RAY_T -- fee:25bp tvl:500K --> USDC
        SOL -- fee:100bp tvl:50K --> ZEC
        ZEC -- fee:30bp tvl:120K --> USDC
    end

    subgraph Pathfinding[Phase 5 - Route Discovery]
        DFS[all_simple_paths DFS]
        EXPAND[Cartesian Expansion]
        DEDUP[Route Deduplication]
        SORT[Sort by heuristic_cost]
    end

    subgraph API[Phase 6 - API Layer]
        ROUTES[GET /api/routes]
        POOLS_EP[GET /api/pools]
    end

    RAY --> POOLS
    MET --> POOLS
    WHP --> POOLS

    POOLS --> F1 --> F2 --> F3 --> F4 --> F5

    F5 --> Graph

    Graph --> DFS --> EXPAND --> DEDUP --> SORT

    SORT --> ROUTES
    POOLS --> POOLS_EP
```
