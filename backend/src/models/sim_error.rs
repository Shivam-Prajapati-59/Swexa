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
