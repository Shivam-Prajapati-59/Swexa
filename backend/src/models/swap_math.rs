use crate::models::sim_error::SimulationError;

const FEE_DENOMINATOR: u128 = 1_000_000;
const NEWTON_MAX_ITERS: usize = 64;
const NEWTON_EPSILON: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapResult {
    pub amount_out: u128,
    pub fee_amount: u128,
    pub price_impact_pct: f64,
    pub is_approximate: bool,
}

#[inline]
pub fn fee_amount(amount: u128, fee_rate_ppm: u32) -> Result<u128, SimulationError> {
    if fee_rate_ppm as u128 >= FEE_DENOMINATOR {
        return Err(SimulationError::FeeExceedsInput);
    }

    amount
        .checked_mul(fee_rate_ppm as u128)
        .ok_or(SimulationError::Overflow)
        .map(|v| v / FEE_DENOMINATOR)
}

#[inline]
fn amount_after_fee(amount: u128, fee_rate_ppm: u32) -> Result<(u128, u128), SimulationError> {
    if amount == 0 {
        return Err(SimulationError::ZeroInput);
    }

    let fee = fee_amount(amount, fee_rate_ppm)?;
    if fee >= amount {
        return Err(SimulationError::FeeExceedsInput);
    }

    Ok((amount - fee, fee))
}

#[inline]
fn price_impact_pct_from_spot(spot_out: f64, amount_out: u128) -> f64 {
    if !spot_out.is_finite() || spot_out <= 0.0 {
        return 0.0;
    }

    let impact = ((spot_out - amount_out as f64) / spot_out) * 100.0;
    if impact.is_finite() {
        impact.max(0.0)
    } else {
        0.0
    }
}

pub fn simulate_cpmm(
    amount_in: u128,
    reserve_in: u128,
    reserve_out: u128,
    fee_rate_ppm: u32,
    is_approximate: bool,
) -> Result<SwapResult, SimulationError> {
    if reserve_in == 0 || reserve_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let (amount_after_fee, fee) = amount_after_fee(amount_in, fee_rate_ppm)?;
    if amount_after_fee == 0 {
        return Err(SimulationError::FeeExceedsInput);
    }

    let numerator = reserve_out
        .checked_mul(amount_after_fee)
        .ok_or(SimulationError::Overflow)?;
    let denominator = reserve_in
        .checked_add(amount_after_fee)
        .ok_or(SimulationError::Overflow)?;
    let amount_out = numerator / denominator;
    if amount_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let spot_out = (amount_after_fee as f64) * (reserve_out as f64) / (reserve_in as f64);

    Ok(SwapResult {
        amount_out,
        fee_amount: fee,
        price_impact_pct: price_impact_pct_from_spot(spot_out, amount_out),
        is_approximate,
    })
}

pub fn simulate_stableswap(
    amount_in: u128,
    reserve_in: u128,
    reserve_out: u128,
    multiplier_in: u64,
    multiplier_out: u64,
    amp_factor: u64,
    fee_rate_ppm: u32,
) -> Result<SwapResult, SimulationError> {
    if reserve_in == 0 || reserve_out == 0 || multiplier_in == 0 || multiplier_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let (amount_after_fee, fee) = amount_after_fee(amount_in, fee_rate_ppm)?;
    let x = (reserve_in as f64) * (multiplier_in as f64);
    let y = (reserve_out as f64) * (multiplier_out as f64);
    let a = amp_factor as f64;

    if x <= 0.0 || y <= 0.0 || a <= 0.0 || !x.is_finite() || !y.is_finite() {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let d = solve_stableswap_d(x, y, a)?;
    let dx_norm = (amount_after_fee as f64) * (multiplier_in as f64);
    let x_new = x + dx_norm;
    if !x_new.is_finite() || x_new <= x {
        return Err(SimulationError::Overflow);
    }

    let y_new = solve_stableswap_y(x_new, y, d, a)?;
    if y_new <= 0.0 || y_new >= y || !y_new.is_finite() {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let dy = (y - y_new) / (multiplier_out as f64);
    if !dy.is_finite() || dy <= 0.0 || dy > u128::MAX as f64 {
        return Err(SimulationError::Overflow);
    }

    let amount_out = dy as u128;
    if amount_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let spot = stableswap_spot_price(x, y, d, a) * (multiplier_in as f64) / (multiplier_out as f64);
    let spot_out = (amount_after_fee as f64) * spot;

    Ok(SwapResult {
        amount_out,
        fee_amount: fee,
        price_impact_pct: price_impact_pct_from_spot(spot_out, amount_out),
        is_approximate: true,
    })
}

fn solve_stableswap_d(x: f64, y: f64, a: f64) -> Result<f64, SimulationError> {
    let mut d = x + y;
    let xy = x * y;
    if xy <= 0.0 || !xy.is_finite() {
        return Err(SimulationError::InsufficientLiquidity);
    }

    for _ in 0..NEWTON_MAX_ITERS {
        let d2 = d * d;
        let d3 = d2 * d;
        let num = 16.0 * a * xy * (x + y) + 2.0 * d3;
        let den = 16.0 * a * xy + 3.0 * d2;
        if den <= 0.0 || !den.is_finite() || !num.is_finite() {
            return Err(SimulationError::NoConvergence);
        }

        let d_next = num / den;
        if !d_next.is_finite() || d_next <= 0.0 {
            return Err(SimulationError::NoConvergence);
        }
        if (d_next - d).abs() <= NEWTON_EPSILON {
            return Ok(d_next);
        }
        d = d_next;
    }

    Err(SimulationError::NoConvergence)
}

fn solve_stableswap_y(x_new: f64, y_initial: f64, d: f64, a: f64) -> Result<f64, SimulationError> {
    let mut y_new = y_initial;
    let d3 = d * d * d;

    for _ in 0..NEWTON_MAX_ITERS {
        let y2 = y_new * y_new;
        let four_xy = 4.0 * x_new * y_new;
        if y2 <= 0.0 || four_xy <= 0.0 || !four_xy.is_finite() {
            return Err(SimulationError::NoConvergence);
        }

        let f_val = d3 / four_xy + 4.0 * a * (x_new + y_new - d) - d;
        let f_prime = -d3 / (4.0 * x_new * y2) + 4.0 * a;
        if f_prime.abs() <= 1e-18 || !f_val.is_finite() || !f_prime.is_finite() {
            return Err(SimulationError::NoConvergence);
        }

        let y_next = y_new - f_val / f_prime;
        if !y_next.is_finite() || y_next <= 0.0 {
            return Err(SimulationError::NoConvergence);
        }
        if (y_next - y_new).abs() <= NEWTON_EPSILON {
            return Ok(y_next);
        }
        y_new = y_next;
    }

    Err(SimulationError::NoConvergence)
}

fn stableswap_spot_price(x: f64, y: f64, d: f64, a: f64) -> f64 {
    let d3 = d * d * d;
    let four_a = 4.0 * a;
    let xy_term = d3 / (4.0 * x * y);
    (four_a + xy_term / x) / (four_a + xy_term / y)
}

pub fn simulate_clmm_virtual_reserves(
    amount_in: u128,
    liquidity: u128,
    sqrt_price_x64: u128,
    a_to_b: bool,
    fee_rate_ppm: u32,
) -> Result<SwapResult, SimulationError> {
    let (virtual_a, virtual_b) = clmm_virtual_reserves(liquidity, sqrt_price_x64)?;
    let (reserve_in, reserve_out) = if a_to_b {
        (virtual_a, virtual_b)
    } else {
        (virtual_b, virtual_a)
    };

    simulate_cpmm(amount_in, reserve_in, reserve_out, fee_rate_ppm, true)
}

fn clmm_virtual_reserves(
    liquidity: u128,
    sqrt_price_x64: u128,
) -> Result<(u128, u128), SimulationError> {
    if liquidity == 0 || sqrt_price_x64 == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    // virtual_a = liquidity * 2^64 / sqrt_price_x64
    // Use widening multiplication to compute (liquidity * 2^64) as a 256-bit value,
    // then divide by sqrt_price_x64. This avoids the truncation bug from dividing
    // before shifting when liquidity >> 64 != 0.
    let virtual_a = {
        // Represent liquidity * 2^64 as (hi, lo) = mul_u128_u128(liquidity, 1 << 64)
        // which is equivalent to: hi = liquidity >> 64, lo = liquidity << 64
        let hi = liquidity >> 64;
        let lo = liquidity << 64; // wraps, which is correct for the low word
        // Now divide the 256-bit number (hi, lo) by sqrt_price_x64
        div_u256_by_u128(hi, lo, sqrt_price_x64)?
    };

    let (hi, lo) = mul_u128_u128(liquidity, sqrt_price_x64);
    let virtual_b = if hi > 0 {
        if hi >> 64 != 0 {
            return Err(SimulationError::Overflow);
        }
        (hi << 64) | (lo >> 64)
    } else {
        lo >> 64
    };

    if virtual_a == 0 || virtual_b == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    Ok((virtual_a, virtual_b))
}

pub fn simulate_dlmm_spot(
    amount_in: u128,
    bin_step: u16,
    active_bin_id: Option<i32>,
    active_price: Option<f64>,
    reserve_in: Option<u128>,
    reserve_out: Option<u128>,
    a_to_b: bool,
    fee_rate_ppm: u32,
) -> Result<SwapResult, SimulationError> {
    let (amount_after_fee, fee) = amount_after_fee(amount_in, fee_rate_ppm)?;
    if let Some(reserve_in) = reserve_in
        && amount_after_fee > reserve_in
    {
        return Err(SimulationError::InsufficientLiquidity);
    }
    let price = if let Some(price) = active_price {
        price
    } else if let Some(active_bin_id) = active_bin_id {
        let base = 1.0 + (bin_step as f64 / 10_000.0);
        base.powf(active_bin_id as f64)
    } else {
        return Err(SimulationError::InsufficientLiquidity);
    };

    if !price.is_finite() || price <= 0.0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let spot = if a_to_b {
        price
    } else if price > 0.0 {
        1.0 / price
    } else {
        return Err(SimulationError::InsufficientLiquidity);
    };

    let amount_out_f = (amount_after_fee as f64) * spot;
    if !amount_out_f.is_finite() || amount_out_f <= 0.0 || amount_out_f > u128::MAX as f64 {
        return Err(SimulationError::Overflow);
    }

    let amount_out = amount_out_f as u128;
    if amount_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }
    if let Some(reserve_out) = reserve_out
        && amount_out > reserve_out
    {
        return Err(SimulationError::InsufficientLiquidity);
    }

    Ok(SwapResult {
        amount_out,
        fee_amount: fee,
        price_impact_pct: 0.0,
        is_approximate: true,
    })
}

fn mul_u128_u128(a: u128, b: u128) -> (u128, u128) {
    let mask = u64::MAX as u128;
    let a0 = a & mask;
    let a1 = a >> 64;
    let b0 = b & mask;
    let b1 = b >> 64;

    let p0 = a0 * b0;
    let p1 = a0 * b1;
    let p2 = a1 * b0;
    let p3 = a1 * b1;

    let middle_low = (p0 >> 64) + (p1 & mask) + (p2 & mask);
    let lo = (p0 & mask) | (middle_low << 64);
    let hi = p3 + (p1 >> 64) + (p2 >> 64) + (middle_low >> 64);

    (hi, lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpmm_uses_integer_math() {
        let result = simulate_cpmm(1_000, 10_000, 20_000, 3_000, false).unwrap();
        let amount_after_fee = 997;
        assert_eq!(result.fee_amount, 3);
        assert_eq!(
            result.amount_out,
            20_000 * amount_after_fee / (10_000 + amount_after_fee)
        );
        assert!(!result.is_approximate);
    }

    #[test]
    fn fee_rejects_excessive_rate() {
        assert_eq!(
            simulate_cpmm(1_000, 10_000, 20_000, 1_000_000, false),
            Err(SimulationError::FeeExceedsInput)
        );
    }

    #[test]
    fn clmm_virtual_reserves_quote() {
        let sqrt_price_x64 = 1u128 << 64;
        let result =
            simulate_clmm_virtual_reserves(1_000, 1_000_000, sqrt_price_x64, true, 0).unwrap();
        assert!(result.amount_out > 0);
        assert!(result.is_approximate);
    }

    #[test]
    fn dlmm_uses_direct_active_price_when_available() {
        let result = simulate_dlmm_spot(
            1_000_000_000,
            4,
            None,
            Some(0.075),
            Some(2_000_000_000),
            Some(100_000_000),
            true,
            400,
        )
        .unwrap();
        assert_eq!(result.fee_amount, 400_000);
        assert_eq!(result.amount_out, 74_970_000);
        assert!(result.is_approximate);
    }

    #[test]
    fn dlmm_rejects_output_above_known_reserve() {
        assert_eq!(
            simulate_dlmm_spot(
                1_000_000_000,
                4,
                None,
                Some(0.075),
                Some(2_000_000_000),
                Some(10),
                true,
                400
            ),
            Err(SimulationError::InsufficientLiquidity)
        );
    }

    #[test]
    fn dlmm_rejects_input_above_known_reserve() {
        assert_eq!(
            simulate_dlmm_spot(
                1_000_000_000,
                4,
                None,
                Some(0.075),
                Some(100),
                Some(100_000_000),
                true,
                400
            ),
            Err(SimulationError::InsufficientLiquidity)
        );
    }
}
