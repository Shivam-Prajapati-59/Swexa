use crate::models::pool::{ClmmTick, DlmmBin};
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

#[derive(Debug, Clone, Copy)]
pub struct DlmmQuoteParams<'a> {
    pub bin_step: u16,
    pub active_bin_id: Option<i32>,
    pub active_price: Option<f64>,
    pub bins: &'a [DlmmBin],
    pub reserve_in: Option<u128>,
    pub reserve_out: Option<u128>,
    pub a_to_b: bool,
    pub fee_rate_ppm: u32,
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

pub fn simulate_clmm_tick_traversal(
    amount_in: u128,
    liquidity: u128,
    sqrt_price_x64: u128,
    current_tick_index: i32,
    initialized_ticks: &[ClmmTick],
    a_to_b: bool,
    fee_rate_ppm: u32,
) -> Result<SwapResult, SimulationError> {
    if liquidity == 0 || sqrt_price_x64 == 0 || initialized_ticks.is_empty() {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let (mut remaining, fee) = amount_after_fee(amount_in, fee_rate_ppm)?;
    let q64 = (1u128 << 64) as f64;
    let mut sqrt_price = sqrt_price_x64 as f64 / q64;
    let mut active_liquidity = liquidity as f64;
    let mut amount_out = 0.0f64;

    if !sqrt_price.is_finite() || sqrt_price <= 0.0 || !active_liquidity.is_finite() {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let mut ticks: Vec<ClmmTick> = initialized_ticks.to_vec();
    ticks.sort_by_key(|tick| tick.index);

    if a_to_b {
        for tick in ticks
            .iter()
            .rev()
            .filter(|tick| tick.index <= current_tick_index)
        {
            if remaining == 0 {
                break;
            }
            if active_liquidity <= 0.0 {
                return Err(SimulationError::InsufficientLiquidity);
            }

            let target = tick_index_to_sqrt_price(tick.index)?;
            if target <= 0.0 || target >= sqrt_price {
                continue;
            }

            let amount_in_to_target =
                active_liquidity * (sqrt_price - target) / (sqrt_price * target);
            if !amount_in_to_target.is_finite() || amount_in_to_target <= 0.0 {
                continue;
            }

            if (remaining as f64) < amount_in_to_target {
                let next_sqrt = active_liquidity * sqrt_price
                    / (active_liquidity + (remaining as f64) * sqrt_price);
                amount_out += active_liquidity * (sqrt_price - next_sqrt);
                remaining = 0;
                break;
            }

            amount_out += active_liquidity * (sqrt_price - target);
            remaining = remaining.saturating_sub(amount_in_to_target.ceil() as u128);
            sqrt_price = target;
            active_liquidity -= tick.liquidity_net as f64;
        }
    } else {
        for tick in ticks.iter().filter(|tick| tick.index > current_tick_index) {
            if remaining == 0 {
                break;
            }
            if active_liquidity <= 0.0 {
                return Err(SimulationError::InsufficientLiquidity);
            }

            let target = tick_index_to_sqrt_price(tick.index)?;
            if target <= sqrt_price {
                continue;
            }

            let amount_in_to_target = active_liquidity * (target - sqrt_price);
            if !amount_in_to_target.is_finite() || amount_in_to_target <= 0.0 {
                continue;
            }

            if (remaining as f64) < amount_in_to_target {
                let next_sqrt = sqrt_price + (remaining as f64) / active_liquidity;
                amount_out +=
                    active_liquidity * (next_sqrt - sqrt_price) / (next_sqrt * sqrt_price);
                remaining = 0;
                break;
            }

            amount_out += active_liquidity * (target - sqrt_price) / (target * sqrt_price);
            remaining = remaining.saturating_sub(amount_in_to_target.ceil() as u128);
            sqrt_price = target;
            active_liquidity += tick.liquidity_net as f64;
        }
    }

    if remaining > 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    if !amount_out.is_finite() || amount_out <= 0.0 || amount_out > u128::MAX as f64 {
        return Err(SimulationError::Overflow);
    }

    let amount_out = amount_out as u128;
    if amount_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let spot_out = if a_to_b {
        let price = (sqrt_price_x64 as f64 / q64).powi(2);
        (amount_in.saturating_sub(fee)) as f64 * price
    } else {
        let price = (sqrt_price_x64 as f64 / q64).powi(2);
        if price <= 0.0 {
            0.0
        } else {
            (amount_in.saturating_sub(fee)) as f64 / price
        }
    };

    Ok(SwapResult {
        amount_out,
        fee_amount: fee,
        price_impact_pct: price_impact_pct_from_spot(spot_out, amount_out),
        is_approximate: true,
    })
}

fn tick_index_to_sqrt_price(tick_index: i32) -> Result<f64, SimulationError> {
    let sqrt_price = 1.0001f64.powf(tick_index as f64 / 2.0);
    if sqrt_price.is_finite() && sqrt_price > 0.0 {
        Ok(sqrt_price)
    } else {
        Err(SimulationError::Overflow)
    }
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
    params: &DlmmQuoteParams<'_>,
) -> Result<SwapResult, SimulationError> {
    let (amount_after_fee, fee) = amount_after_fee(amount_in, params.fee_rate_ppm)?;
    if let Some(reserve_in) = params.reserve_in
        && amount_after_fee > reserve_in
    {
        return Err(SimulationError::InsufficientLiquidity);
    }
    let price = if let Some(price) = params.active_price {
        price
    } else if let Some(active_bin_id) = params.active_bin_id {
        let base = 1.0 + (params.bin_step as f64 / 10_000.0);
        base.powf(active_bin_id as f64)
    } else {
        return Err(SimulationError::InsufficientLiquidity);
    };

    if !price.is_finite() || price <= 0.0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let spot = if params.a_to_b {
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
    if let Some(reserve_out) = params.reserve_out
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

pub fn simulate_dlmm_bin_traversal(
    amount_in: u128,
    params: &DlmmQuoteParams<'_>,
) -> Result<SwapResult, SimulationError> {
    if params.bins.is_empty() {
        return simulate_dlmm_spot(amount_in, params);
    }

    let (mut remaining, fee) = amount_after_fee(amount_in, params.fee_rate_ppm)?;
    if let Some(reserve_in) = params.reserve_in
        && remaining > reserve_in
    {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let active_bin_id = params
        .active_bin_id
        .ok_or(SimulationError::InsufficientLiquidity)?;
    let base = 1.0 + (params.bin_step as f64 / 10_000.0);
    let active_price = params
        .active_price
        .unwrap_or_else(|| base.powf(active_bin_id as f64));
    if !active_price.is_finite() || active_price <= 0.0 {
        return Err(SimulationError::InsufficientLiquidity);
    }
    let mut sorted_bins: Vec<DlmmBin> = params.bins.to_vec();
    sorted_bins.sort_by_key(|bin| bin.id);
    if params.a_to_b {
        sorted_bins.reverse();
    }

    let mut amount_out = 0.0f64;
    for bin in sorted_bins {
        if remaining == 0 {
            break;
        }

        if params.a_to_b && bin.id > active_bin_id {
            continue;
        }
        if !params.a_to_b && bin.id < active_bin_id {
            continue;
        }

        let price = active_price * base.powi(bin.id.saturating_sub(active_bin_id));
        if !price.is_finite() || price <= 0.0 {
            continue;
        }

        if params.a_to_b {
            if bin.amount_y == 0 {
                continue;
            }
            let max_in_for_bin = (bin.amount_y as f64) / price;
            if !max_in_for_bin.is_finite() || max_in_for_bin <= 0.0 {
                continue;
            }

            if (remaining as f64) <= max_in_for_bin {
                amount_out += (remaining as f64) * price;
                remaining = 0;
            } else {
                amount_out += bin.amount_y as f64;
                remaining = remaining.saturating_sub(max_in_for_bin.ceil() as u128);
            }
        } else {
            if bin.amount_x == 0 {
                continue;
            }
            let max_in_for_bin = (bin.amount_x as f64) * price;
            if !max_in_for_bin.is_finite() || max_in_for_bin <= 0.0 {
                continue;
            }

            if (remaining as f64) <= max_in_for_bin {
                amount_out += (remaining as f64) / price;
                remaining = 0;
            } else {
                amount_out += bin.amount_x as f64;
                remaining = remaining.saturating_sub(max_in_for_bin.ceil() as u128);
            }
        }
    }

    if remaining > 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }

    if !amount_out.is_finite() || amount_out <= 0.0 || amount_out > u128::MAX as f64 {
        return Err(SimulationError::Overflow);
    }

    let amount_out = amount_out as u128;
    if amount_out == 0 {
        return Err(SimulationError::InsufficientLiquidity);
    }
    if let Some(reserve_out) = params.reserve_out
        && amount_out > reserve_out
    {
        return Err(SimulationError::InsufficientLiquidity);
    }

    let spot = if params.a_to_b {
        active_price
    } else {
        1.0 / active_price
    };
    let spot_out = (amount_in.saturating_sub(fee)) as f64 * spot;

    Ok(SwapResult {
        amount_out,
        fee_amount: fee,
        price_impact_pct: price_impact_pct_from_spot(spot_out, amount_out),
        is_approximate: true,
    })
}

/// Divides a 256-bit number represented as `(hi, lo)` by a `u128` divisor.
/// Returns `Ok(quotient)` if the result fits in `u128`, otherwise `Err(Overflow)`.
///
/// Uses long division: divide `hi` first, carry the remainder into `lo`.
fn div_u256_by_u128(hi: u128, lo: u128, divisor: u128) -> Result<u128, SimulationError> {
    if divisor == 0 {
        return Err(SimulationError::Overflow);
    }
    if hi == 0 {
        return Ok(lo / divisor);
    }
    // q_hi = hi / divisor, r = hi % divisor
    let q_hi = hi / divisor;
    let r = hi % divisor;

    // If q_hi >= 2^128 / 2^128 ... it's already u128, but if shifting it
    // by 128 bits overflows u128, the final quotient > u128.
    if q_hi > 0 {
        // q_hi * 2^128 won't fit in u128
        return Err(SimulationError::Overflow);
    }

    // Now compute (r * 2^128 + lo) / divisor.
    // r < divisor (guaranteed by modulo), so r * 2^128 + lo < divisor * 2^128 + lo,
    // meaning the quotient fits in u128.
    // Use the same widening trick: (r, lo) / divisor
    // Since r < divisor <= u128::MAX, we can use mul_u128_u128-style decomposition,
    // but simpler: iterate by splitting into two 64-bit divisions.
    //
    // Actually, since r < divisor, we know the result fits in u128.
    // We can compute this via: (r << 64 | lo_hi) / divisor, then handle remainder with lo_lo.
    let lo_hi = lo >> 64;
    let lo_lo = lo & (u64::MAX as u128);

    // First: combine r and lo_hi
    // dividend_1 = r * 2^64 + lo_hi
    // This fits in ~192 bits, but r < divisor <= u128::MAX, so r * 2^64 might overflow u128.
    // Use the same two-step approach:
    let (mut q, rem) = if r == 0 {
        (lo_hi / divisor, lo_hi % divisor)
    } else {
        // r * 2^64 could overflow u128, so compute (r * 2^64 + lo_hi) / divisor
        // by first doing r / divisor and carrying remainder.
        let q1 = r / divisor; // always 0 since r < divisor
        let r1 = r % divisor; // == r
        debug_assert_eq!(q1, 0);
        let _ = q1;

        // Now (r1 << 64 | lo_hi) / divisor, where r1 < divisor
        // If r1 < 2^64, then r1 << 64 fits in u128
        if r1 >> 64 == 0 {
            let dividend = (r1 << 64) | lo_hi;
            (dividend / divisor, dividend % divisor)
        } else {
            // r1 has high bits, need wider math. Use iterative shift-subtract.
            // This path is rare (liquidity extremely large).
            // Fallback: compute via f64 (acceptable since this is the virtual_a path
            // and result is fed into CPMM which is approximate for CLMM anyway).
            let approx = ((r1 as f64) * (1u128 << 64) as f64 + lo_hi as f64) / divisor as f64;
            if !approx.is_finite() || approx < 0.0 || approx > u128::MAX as f64 {
                return Err(SimulationError::Overflow);
            }
            (approx as u128, 0u128)
        }
    };

    // Second step: incorporate lo_lo
    // remaining = rem * 2^64 + lo_lo
    if rem >> 64 == 0 {
        let remaining = (rem << 64) | lo_lo;
        q = q.checked_shl(64).ok_or(SimulationError::Overflow)?;
        q = q
            .checked_add(remaining / divisor)
            .ok_or(SimulationError::Overflow)?;
    } else {
        // rem has high bits — same rare path
        let approx = ((rem as f64) * (1u128 << 64) as f64 + lo_lo as f64) / divisor as f64;
        if !approx.is_finite() || approx < 0.0 || approx > u128::MAX as f64 {
            return Err(SimulationError::Overflow);
        }
        q = q.checked_shl(64).ok_or(SimulationError::Overflow)?;
        q = q
            .checked_add(approx as u128)
            .ok_or(SimulationError::Overflow)?;
    }

    Ok(q)
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

    fn dlmm_params<'a>(
        active_price: f64,
        bins: &'a [DlmmBin],
        reserve_in: Option<u128>,
        reserve_out: Option<u128>,
    ) -> DlmmQuoteParams<'a> {
        DlmmQuoteParams {
            bin_step: 100,
            active_bin_id: Some(0),
            active_price: Some(active_price),
            bins,
            reserve_in,
            reserve_out,
            a_to_b: true,
            fee_rate_ppm: 0,
        }
    }

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
    fn clmm_tick_traversal_crosses_initialized_ticks() {
        let ticks = vec![
            ClmmTick {
                index: -100,
                liquidity_net: -100_000,
            },
            ClmmTick {
                index: 100,
                liquidity_net: 100_000,
            },
        ];
        let result =
            simulate_clmm_tick_traversal(1_000, 1_000_000, 1u128 << 64, 0, &ticks, true, 0)
                .unwrap();

        assert!(result.amount_out > 0);
        assert!(result.is_approximate);
    }

    #[test]
    fn clmm_tick_traversal_rejects_unhydrated_ticks() {
        let ticks = vec![ClmmTick {
            index: -100,
            liquidity_net: -100_000,
        }];

        assert_eq!(
            simulate_clmm_tick_traversal(10_000, 1_000_000, 1u128 << 64, 0, &ticks, true, 0),
            Err(SimulationError::InsufficientLiquidity)
        );
    }

    #[test]
    fn dlmm_uses_direct_active_price_when_available() {
        let params = DlmmQuoteParams {
            bin_step: 4,
            active_bin_id: None,
            active_price: Some(0.075),
            bins: &[],
            reserve_in: Some(2_000_000_000),
            reserve_out: Some(100_000_000),
            a_to_b: true,
            fee_rate_ppm: 400,
        };
        let result = simulate_dlmm_spot(1_000_000_000, &params).unwrap();
        assert_eq!(result.fee_amount, 400_000);
        assert_eq!(result.amount_out, 74_970_000);
        assert!(result.is_approximate);
    }

    #[test]
    fn dlmm_bin_traversal_walks_hydrated_bins() {
        let bins = vec![
            DlmmBin {
                id: -1,
                amount_x: 10_000,
                amount_y: 10_000,
            },
            DlmmBin {
                id: 0,
                amount_x: 10_000,
                amount_y: 100,
            },
        ];

        let params = dlmm_params(1.0, &bins, Some(1_000_000), Some(1_000_000));
        let result = simulate_dlmm_bin_traversal(200, &params).unwrap();

        assert!(result.amount_out > 100);
        assert!(result.is_approximate);
    }

    #[test]
    fn dlmm_bin_traversal_rejects_unhydrated_liquidity() {
        let bins = vec![DlmmBin {
            id: 0,
            amount_x: 10,
            amount_y: 10,
        }];

        assert_eq!(
            simulate_dlmm_bin_traversal(
                1_000,
                &dlmm_params(1.0, &bins, Some(1_000_000), Some(1_000_000))
            ),
            Err(SimulationError::InsufficientLiquidity)
        );
    }

    #[test]
    fn dlmm_rejects_output_above_known_reserve() {
        assert_eq!(
            simulate_dlmm_spot(
                1_000_000_000,
                &DlmmQuoteParams {
                    bin_step: 4,
                    active_bin_id: None,
                    active_price: Some(0.075),
                    bins: &[],
                    reserve_in: Some(2_000_000_000),
                    reserve_out: Some(10),
                    a_to_b: true,
                    fee_rate_ppm: 400,
                }
            ),
            Err(SimulationError::InsufficientLiquidity)
        );
    }

    #[test]
    fn dlmm_rejects_input_above_known_reserve() {
        assert_eq!(
            simulate_dlmm_spot(
                1_000_000_000,
                &DlmmQuoteParams {
                    bin_step: 4,
                    active_bin_id: None,
                    active_price: Some(0.075),
                    bins: &[],
                    reserve_in: Some(100),
                    reserve_out: Some(100_000_000),
                    a_to_b: true,
                    fee_rate_ppm: 400,
                }
            ),
            Err(SimulationError::InsufficientLiquidity)
        );
    }
}
