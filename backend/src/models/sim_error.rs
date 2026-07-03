#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationError {
    MismatchedMint,
    ZeroInput,
    FeeExceedsInput,
    InsufficientLiquidity,
    NoConvergence,
    Overflow,
    UnsupportedPoolType,
}

impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            SimulationError::MismatchedMint => "input mint is not in the pool",
            SimulationError::ZeroInput => "swap input amount is zero",
            SimulationError::FeeExceedsInput => "fee exceeds or consumes swap input",
            SimulationError::InsufficientLiquidity => "insufficient liquidity",
            SimulationError::NoConvergence => "simulation did not converge",
            SimulationError::Overflow => "simulation arithmetic overflow",
            SimulationError::UnsupportedPoolType => "pool type is unsupported",
        };
        f.write_str(message)
    }
}

impl std::error::Error for SimulationError {}
