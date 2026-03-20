use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, TokenAccount};

pub mod adapters;

declare_id!("D5WaQNE3jj8NJ97hq9Y38xansEerSrbLiohGRjexZC3p");

#[program]
pub mod cross_swap {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }

    pub fn execute_whirlpool_swap(
        ctx: Context<ExecuteWhirlpoolSwap>,
        amount_in: u64,
    ) -> Result<()> {
        let accounts = adapters::whirlpool::WhirlpoolAccounts {
            whirlpool_program: ctx.accounts.whirlpool_program.to_account_info(),
            token_program: ctx.accounts.token_program.to_account_info(),
            token_authority: ctx.accounts.token_authority.to_account_info(),
            whirlpool: ctx.accounts.whirlpool.to_account_info(),
            token_owner_account_a: ctx.accounts.token_owner_account_a.to_account_info(),
            token_vault_a: ctx.accounts.token_vault_a.to_account_info(),
            token_owner_account_b: ctx.accounts.token_owner_account_b.to_account_info(),
            token_vault_b: ctx.accounts.token_vault_b.to_account_info(),
            tick_array0: ctx.accounts.tick_array0.to_account_info(),
            tick_array1: ctx.accounts.tick_array1.to_account_info(),
            tick_array2: ctx.accounts.tick_array2.to_account_info(),
            oracle: ctx.accounts.oracle.to_account_info(),
        };

        adapters::whirlpool::swap(
            &accounts,
            &ctx.accounts.swap_source_mint.key(),
            &ctx.accounts.swap_destination_mint.key(),
            &ctx.accounts.token_vault_a_mint.key(),
            &ctx.accounts.token_vault_b_mint.key(),
            amount_in,
        )?;

        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}

#[derive(Accounts)]
pub struct ExecuteWhirlpoolSwap<'info> {
    /// CHECK: Orca program
    pub whirlpool_program: AccountInfo<'info>,
    /// CHECK: SPL Token program
    pub token_program: AccountInfo<'info>,
    pub token_authority: Signer<'info>,

    /// CHECK: Whirlpool state
    #[account(mut)]
    pub whirlpool: AccountInfo<'info>,

    /// CHECK: User ATA A
    #[account(mut)]
    pub token_owner_account_a: AccountInfo<'info>,
    /// CHECK: Vault A
    #[account(mut)]
    pub token_vault_a: AccountInfo<'info>,

    /// CHECK: User ATA B
    #[account(mut)]
    pub token_owner_account_b: AccountInfo<'info>,
    /// CHECK: Vault B
    #[account(mut)]
    pub token_vault_b: AccountInfo<'info>,

    /// CHECK: TickArrays
    #[account(mut)]
    pub tick_array0: AccountInfo<'info>,
    /// CHECK: TickArrays
    #[account(mut)]
    pub tick_array1: AccountInfo<'info>,
    /// CHECK: TickArrays
    #[account(mut)]
    pub tick_array2: AccountInfo<'info>,

    /// CHECK: Oracle
    pub oracle: AccountInfo<'info>,

    // Explicit mints to safely check directions
    /// CHECK: source mint
    pub swap_source_mint: AccountInfo<'info>,
    /// CHECK: dest mint
    pub swap_destination_mint: AccountInfo<'info>,
    /// CHECK: vault a mint
    pub token_vault_a_mint: AccountInfo<'info>,
    /// CHECK: vault b mint
    pub token_vault_b_mint: AccountInfo<'info>,
}
