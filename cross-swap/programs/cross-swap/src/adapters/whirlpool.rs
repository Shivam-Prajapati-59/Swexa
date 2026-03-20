use anchor_lang::{
    prelude::*, solana_program::instruction::Instruction, solana_program::program::invoke,
};

pub const ACCOUNTS_LEN: usize = 14;
pub const SWAP_SELECTOR: &[u8] = &[248, 198, 158, 145, 225, 117, 135, 200];

pub struct WhirlpoolAccounts<'info> {
    pub whirlpool_program: AccountInfo<'info>,
    pub token_program: AccountInfo<'info>,
    pub token_authority: AccountInfo<'info>,
    pub whirlpool: AccountInfo<'info>,
    pub token_owner_account_a: AccountInfo<'info>,
    pub token_vault_a: AccountInfo<'info>,
    pub token_owner_account_b: AccountInfo<'info>,
    pub token_vault_b: AccountInfo<'info>,
    pub tick_array0: AccountInfo<'info>,
    pub tick_array1: AccountInfo<'info>,
    pub tick_array2: AccountInfo<'info>,
    pub oracle: AccountInfo<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SwapArgs {
    pub amount: u64,
    pub other_amount_threshold: u64,
    pub sqrt_price_limit: u128,
    pub amount_specified_is_input: bool,
    pub a_to_b: bool,
}

pub fn swap<'a>(
    accounts: &WhirlpoolAccounts<'a>,
    swap_source_mint: &Pubkey,
    swap_destination_mint: &Pubkey,
    vault_a_mint: &Pubkey,
    vault_b_mint: &Pubkey,
    amount_in: u64,
) -> Result<()> {
    let amount_specified_is_input = true;
    let other_amount_threshold = 1u64;

    let a_to_b = if swap_source_mint == vault_a_mint && swap_destination_mint == vault_b_mint {
        true
    } else if swap_source_mint == vault_b_mint && swap_destination_mint == vault_a_mint {
        false
    } else {
        return Err(ProgramError::InvalidInstructionData.into()); // Mint mismatch
    };

    // Limits
    let sqrt_price_limit: u128 = if a_to_b {
        4295048016 // MIN
    } else {
        79226673515401279992447579055 // MAX
    };

    let args = SwapArgs {
        amount: amount_in,
        other_amount_threshold,
        sqrt_price_limit,
        amount_specified_is_input,
        a_to_b,
    };

    let mut data = SWAP_SELECTOR.to_vec();
    args.serialize(&mut data)?;

    let account_infos = vec![
        accounts.token_program.clone(),
        accounts.token_authority.clone(),
        accounts.whirlpool.clone(),
        accounts.token_owner_account_a.clone(),
        accounts.token_vault_a.clone(),
        accounts.token_owner_account_b.clone(),
        accounts.token_vault_b.clone(),
        accounts.tick_array0.clone(),
        accounts.tick_array1.clone(),
        accounts.tick_array2.clone(),
        accounts.oracle.clone(),
    ];

    let instruction = Instruction {
        program_id: accounts.whirlpool_program.key(),
        accounts: vec![
            AccountMeta::new_readonly(accounts.token_program.key(), false),
            AccountMeta::new_readonly(accounts.token_authority.key(), true),
            AccountMeta::new(accounts.whirlpool.key(), false),
            AccountMeta::new(accounts.token_owner_account_a.key(), false),
            AccountMeta::new(accounts.token_vault_a.key(), false),
            AccountMeta::new(accounts.token_owner_account_b.key(), false),
            AccountMeta::new(accounts.token_vault_b.key(), false),
            AccountMeta::new(accounts.tick_array0.key(), false),
            AccountMeta::new(accounts.tick_array1.key(), false),
            AccountMeta::new(accounts.tick_array2.key(), false),
            AccountMeta::new_readonly(accounts.oracle.key(), false),
        ],
        data,
    };

    invoke(&instruction, &account_infos)?;

    Ok(())
}
