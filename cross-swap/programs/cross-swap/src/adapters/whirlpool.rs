use anchor_lang::{prelude::*, solana_program::instruction::Instruction};
use anchor_spl::token_interface::{InterfaceAccount, TokenAccount};
use arrayref::array_ref;

pub const ACCOUNTS_LEN: usize = 14;

pub struct WhirlpoolAccounts<'info> {
    pub token_program: &'info AccountInfo<'info>,
    pub swap_authority_pubkey: &'info AccountInfo<'info>, // signer (or PDA)
    pub swap_source_token: InterfaceAccount<'info, TokenAccount>,
    pub swap_destination_token: InterfaceAccount<'info, TokenAccount>,

    pub token_authority: &'info AccountInfo<'info>,
    pub whirlpool: &'info AccountInfo<'info>,
    pub token_owner_account_a: InterfaceAccount<'info, TokenAccount>,
    pub token_vault_a: InterfaceAccount<'info, TokenAccount>,
    pub token_owner_account_b: InterfaceAccount<'info, TokenAccount>,
    pub token_vault_b: InterfaceAccount<'info, TokenAccount>,

    pub tick_array0: &'info AccountInfo<'info>,
    pub tick_array1: &'info AccountInfo<'info>,
    pub tick_array2: &'info AccountInfo<'info>,
    pub oracle: &'info AccountInfo<'info>, // PDA (readonly/writable depending)
}

impl<'info> WhirlpoolAccounts<'info> {
    pub fn parse_accounts(accounts: &'info [AccountInfo<'info>], offset: usize) -> Result<Self> {
        // ACCOUNTS_LEN must equal number of elements below (14)
        let [
            token_program,
            swap_authority_pubkey,
            swap_source_token,
            swap_destination_token,
            token_authority,
            whirlpool,
            token_owner_account_a,
            token_vault_a,
            token_owner_account_b,
            token_vault_b,
            tick_array0,
            tick_array1,
            tick_array2,
            oracle,
        ]: &[AccountInfo<'info>; ACCOUNTS_LEN] = array_ref![accounts, offset, ACCOUNTS_LEN];

        Ok(Self {
            token_program,
            swap_authority_pubkey,
            swap_source_token: InterfaceAccount::try_from(swap_source_token)?,
            swap_destination_token: InterfaceAccount::try_from(swap_destination_token)?,
            token_authority,
            whirlpool,
            token_owner_account_a: InterfaceAccount::try_from(token_owner_account_a)?,
            token_vault_a: InterfaceAccount::try_from(token_vault_a)?,
            token_owner_account_b: InterfaceAccount::try_from(token_owner_account_b)?,
            token_vault_b: InterfaceAccount::try_from(token_vault_b)?,
            tick_array0,
            tick_array1,
            tick_array2,
            oracle,
        })
    }
}

pub fn swap<'a>(remaining_accounts: &'a [AccountInfo<'a>], amount_in: u64) {
    let amount_specified_is_input = true;
    let other_amount_threshold = 1u64;
    let a_to_b: bool;
    let sqrt_price_limit: u128;

    let amount_specified_is_input = true;
    let other_amount_threshold = 1u64;
    let a_to_b: bool;
    let sqrt_price_limit: i128;

    if swap_accounts.swap_source_token.mint == swap_accounts.token_vault_a.mint
        && swap_accounts.swap_destination_token.mint == swap_accounts.token_vault_b.mint
    {
        a_to_b = true;
        sqrt_price_limit = 4295048016; //The minimum sqrt-price supported by the Whirlpool program.
    } else if swap_accounts.swap_source_token.mint == swap_accounts.token_vault_b.mint
        && swap_accounts.swap_destination_token.mint == swap_accounts.token_vault_a.mint
    {
        a_to_b = false;
        sqrt_price_limit = 79226673515401279992447579055; //The maximum sqrt-price supported by the Whirlpool program.
    } else {
        return Err(ErrorCode::InvalidTokenMint.into());
    }
    let (token_owner_account_a, token_owner_account_b) = if a_to_b {
        (
            swap_accounts.swap_source_token.clone(),
            swap_accounts.swap_destination_token.clone(),
        )
    } else {
        (
            swap_accounts.swap_destination_token.clone(),
            swap_accounts.swap_source_token.clone(),
        )
    };
    let account_infos = [
        swap_accounts.token_program.to_account_info(),
        swap_accounts.swap_authority_pubkey.to_account_info(),
        swap_accounts.whirlpool.to_account_info(),
        token_owner_account_a.to_account_info(),
        swap_accounts.token_vault_a.to_account_info(),
        token_owner_account_b.to_account_info(),
        swap_accounts.token_vault_b.to_account_info(),
        swap_accounts.tick_array0.to_account_info(),
        swap_accounts.tick_array1.to_account_info(),
        swap_accounts.tick_array2.to_account_info(),
        swap_accounts.oracle.to_account_info(),
    ];

    let instruction = Instruction {
        program_id: swap_accounts.dex_program_id.key(),
        accounts,
        data,
    };

    let dex_processor = &WhirlpoolProcessor;
    let amount_out = invoke_process(
        amount_in,
        dex_processor,
        &account_infos,
        &mut swap_accounts.swap_source_token,
        &mut swap_accounts.swap_destination_token,
        hop_accounts,
        instruction,
        hop,
        offset,
        ACCOUNTS_LEN,
        proxy_swap,
        owner_seeds,
    )?;
    Ok(amount_out)
}
