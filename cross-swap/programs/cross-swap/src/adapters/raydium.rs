use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke,
    },
};
use anchor_spl::token::Token;

const RAYDIUM_CLMM_SWAP_SELECTOR: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
const RAYDIUM_CPMM_SWAP_SELECTOR: [u8; 8] = [43, 190, 90, 218, 196, 30, 51, 222];
const ARGS_CLMM_LEN: usize = 41;
const ARGS_CPMM_LEN: usize = 24;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RaydiumClmmSwapArgs {
    pub amount: u64,
    pub other_amount_threshold: u64,
    pub sqrt_price_limit_x64: u128,
    pub is_base_input: bool,
}

fn account_meta_from_info(account_info: &AccountInfo<'_>) -> AccountMeta {
    if account_info.is_writable {
        AccountMeta::new(account_info.key(), account_info.is_signer)
    } else {
        AccountMeta::new_readonly(account_info.key(), account_info.is_signer)
    }
}

pub mod raydium_amm_swap {
    use super::*;

    pub fn execute_swap(
        ctx: Context<RaydiumAmmSwap>,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<()> {
        // 1. Data Preparation (swapBaseIn = 9)
        let mut data = Vec::with_capacity(17);
        data.push(9); // The instruction index
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());

        // 2. Instruction Accounts (Order is strict!)
        let accounts = vec![
            AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
            AccountMeta::new(ctx.accounts.amm.key(), false),
            AccountMeta::new_readonly(ctx.accounts.amm_authority.key(), false),
            AccountMeta::new(ctx.accounts.amm_open_orders.key(), false),
            AccountMeta::new(ctx.accounts.amm_target_orders.key(), false),
            AccountMeta::new(ctx.accounts.pool_coin_token_account.key(), false),
            AccountMeta::new(ctx.accounts.pool_pc_token_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.serum_program.key(), false),
            AccountMeta::new(ctx.accounts.serum_market.key(), false),
            AccountMeta::new(ctx.accounts.serum_bids.key(), false),
            AccountMeta::new(ctx.accounts.serum_asks.key(), false),
            AccountMeta::new(ctx.accounts.serum_event_queue.key(), false),
            AccountMeta::new(ctx.accounts.serum_coin_vault_account.key(), false),
            AccountMeta::new(ctx.accounts.serum_pc_vault_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.serum_vault_signer.key(), false),
            AccountMeta::new(ctx.accounts.user_source_token_account.key(), false),
            AccountMeta::new(ctx.accounts.user_destination_token_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.user_source_owner.key(), true),
        ];

        // 3. Construct the CPI Instruction
        let ix = Instruction {
            program_id: ctx.accounts.raydium_program.key(),
            accounts,
            data,
        };

        // 4. Invoke CPI
        // Note: The program being called (raydium_program) MUST be included in this array.
        invoke(
            &ix,
            &[
                ctx.accounts.token_program.to_account_info(),
                ctx.accounts.amm.to_account_info(),
                ctx.accounts.amm_authority.to_account_info(),
                ctx.accounts.amm_open_orders.to_account_info(),
                ctx.accounts.amm_target_orders.to_account_info(),
                ctx.accounts.pool_coin_token_account.to_account_info(),
                ctx.accounts.pool_pc_token_account.to_account_info(),
                ctx.accounts.serum_program.to_account_info(),
                ctx.accounts.serum_market.to_account_info(),
                ctx.accounts.serum_bids.to_account_info(),
                ctx.accounts.serum_asks.to_account_info(),
                ctx.accounts.serum_event_queue.to_account_info(),
                ctx.accounts.serum_coin_vault_account.to_account_info(),
                ctx.accounts.serum_pc_vault_account.to_account_info(),
                ctx.accounts.serum_vault_signer.to_account_info(),
                ctx.accounts.user_source_token_account.to_account_info(),
                ctx.accounts
                    .user_destination_token_account
                    .to_account_info(),
                ctx.accounts.user_source_owner.to_account_info(),
                ctx.accounts.raydium_program.to_account_info(), // ADDED: Required for invoke
            ],
        )?;

        Ok(())
    }
}

pub mod raydium_clmm_swap {
    use super::*;

    pub fn execute_swap<'info>(
        ctx: Context<'_, '_, '_, 'info, RaydiumClmmSwap<'info>>,
        amount: u64,
        other_amount_threshold: u64,
        sqrt_price_limit_x64: u128,
        is_base_input: bool,
    ) -> Result<()> {
        let args = RaydiumClmmSwapArgs {
            amount,
            other_amount_threshold,
            sqrt_price_limit_x64,
            is_base_input,
        };

        let mut data = Vec::with_capacity(41);
        data.extend_from_slice(&RAYDIUM_CLMM_SWAP_SELECTOR);
        args.serialize(&mut data)?;

        let mut accounts = vec![
            AccountMeta::new_readonly(ctx.accounts.payer.key(), true),
            AccountMeta::new_readonly(ctx.accounts.amm_config.key(), false),
            AccountMeta::new(ctx.accounts.pool_state.key(), false),
            AccountMeta::new(ctx.accounts.input_token_account.key(), false),
            AccountMeta::new(ctx.accounts.output_token_account.key(), false),
            AccountMeta::new(ctx.accounts.input_vault.key(), false),
            AccountMeta::new(ctx.accounts.output_vault.key(), false),
            AccountMeta::new(ctx.accounts.observation_state.key(), false),
            AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
            AccountMeta::new(ctx.accounts.tick_array.key(), false),
        ];
        accounts.extend(ctx.remaining_accounts.iter().map(account_meta_from_info));

        let instruction = Instruction {
            program_id: ctx.accounts.raydium_clmm_program.key(),
            accounts,
            data,
        };

        let mut account_infos = vec![
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.amm_config.to_account_info(),
            ctx.accounts.pool_state.to_account_info(),
            ctx.accounts.input_token_account.to_account_info(),
            ctx.accounts.output_token_account.to_account_info(),
            ctx.accounts.input_vault.to_account_info(),
            ctx.accounts.output_vault.to_account_info(),
            ctx.accounts.observation_state.to_account_info(),
            ctx.accounts.token_program.to_account_info(),
            ctx.accounts.tick_array.to_account_info(),
        ];
        account_infos.extend(ctx.remaining_accounts.iter().cloned());
        account_infos.push(ctx.accounts.raydium_clmm_program.to_account_info());

        invoke(&instruction, &account_infos)?;

        Ok(())
    }
}

#[derive(Accounts)]
pub struct RaydiumAmmSwap<'info> {
    /// CHECK: The Raydium V4 Program ID
    pub raydium_program: AccountInfo<'info>,

    /// CHECK: Raydium SPL Token Program
    pub token_program: AccountInfo<'info>,

    /// CHECK: Raydium AMM Account
    #[account(mut)]
    pub amm: AccountInfo<'info>,

    /// CHECK: Raydium AMM Authority
    pub amm_authority: AccountInfo<'info>,

    /// CHECK: Raydium AMM Open Orders
    #[account(mut)]
    pub amm_open_orders: AccountInfo<'info>,

    /// CHECK: Raydium AMM Target Orders
    #[account(mut)]
    pub amm_target_orders: AccountInfo<'info>,

    /// CHECK: Pool Coin Vault
    #[account(mut)]
    pub pool_coin_token_account: AccountInfo<'info>,

    /// CHECK: Pool PC Vault
    #[account(mut)]
    pub pool_pc_token_account: AccountInfo<'info>,

    /// CHECK: Serum Program ID
    pub serum_program: AccountInfo<'info>,

    /// CHECK: Serum Market
    #[account(mut)]
    pub serum_market: AccountInfo<'info>,

    /// CHECK: Serum Bids
    #[account(mut)]
    pub serum_bids: AccountInfo<'info>,

    /// CHECK: Serum Asks
    #[account(mut)]
    pub serum_asks: AccountInfo<'info>,

    /// CHECK: Serum Event Queue
    #[account(mut)]
    pub serum_event_queue: AccountInfo<'info>,

    /// CHECK: Serum Coin Vault
    #[account(mut)]
    pub serum_coin_vault_account: AccountInfo<'info>,

    /// CHECK: Serum PC Vault
    #[account(mut)]
    pub serum_pc_vault_account: AccountInfo<'info>,

    /// CHECK: Serum Vault Signer
    pub serum_vault_signer: AccountInfo<'info>,

    /// CHECK: User Source Account
    #[account(mut)]
    pub user_source_token_account: AccountInfo<'info>,

    /// CHECK: User Destination Account
    #[account(mut)]
    pub user_destination_token_account: AccountInfo<'info>,

    pub user_source_owner: Signer<'info>,
}

#[derive(Accounts)]
pub struct RaydiumClmmSwap<'info> {
    /// CHECK: Raydium CLMM program account.
    pub raydium_clmm_program: UncheckedAccount<'info>,

    pub payer: Signer<'info>,

    /// CHECK: Raydium amm config account.
    pub amm_config: UncheckedAccount<'info>,

    /// CHECK: Raydium pool state account.
    #[account(mut)]
    pub pool_state: UncheckedAccount<'info>,

    /// CHECK: User input token account.
    #[account(mut)]
    pub input_token_account: UncheckedAccount<'info>,

    /// CHECK: User output token account.
    #[account(mut)]
    pub output_token_account: UncheckedAccount<'info>,

    /// CHECK: Pool input vault.
    #[account(mut)]
    pub input_vault: UncheckedAccount<'info>,

    /// CHECK: Pool output vault.
    #[account(mut)]
    pub output_vault: UncheckedAccount<'info>,

    /// CHECK: Raydium observation state account.
    #[account(mut)]
    pub observation_state: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,

    /// CHECK: The first required tick array for the swap path.
    #[account(mut)]
    pub tick_array: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct RaydiumCpmmSwap<'info> {
    /// CHECK: Raydium CLMM program account.
    pub raydium_clmm_program: AccountInfo<'info>,
    pub amm_config: AccountInfo<'info>,
    pub pool_state: AccountInfo<'info>,
    pub input_vault: AccountInfo<'info>,
    pub output_vault: AccountInfo<'info>,
    pub input_token_program: AccountInfo<'info>,
    pub output_token_program: AccountInfo<'info>,
    pub input_token_mint: AccountInfo<'info>,
    pub output_token_mint: AccountInfo<'info>,
    pub observation_state: AccountInfo<'info>,
}
