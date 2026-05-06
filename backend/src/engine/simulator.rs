use anyhow::{Result, anyhow};
use litesvm::LiteSVM;
use solana_account::{Account, AccountSharedData};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_program_pack::Pack;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use spl_token_interface::state::Account as SplTokenAccount;

pub struct QuoteSimulator {
    svm: LiteSVM,
}

pub struct SimulationQuote {
    pub amount_out: u64,
    pub final_target_balance: u64,
}

impl QuoteSimulator {
    pub fn new() -> Self {
        Self {
            svm: LiteSVM::new(),
        }
    }

    pub fn load_accounts<I>(&mut self, accounts: I) -> Result<()>
    where
        I: IntoIterator<Item = (Address, Account)>,
    {
        for (pubkey, account) in accounts {
            self.svm.set_account(pubkey, account)?;
        }
        Ok(())
    }

    pub fn airdrop(&mut self, address: &Address, lamports: u64) -> Result<()> {
        self.svm
            .airdrop(address, lamports)
            .map(|_| ())
            .map_err(|err| anyhow!("airdrop failed: {err:?}"))
    }

    pub fn simulate_transaction(
        &self,
        instructions: &[Instruction],
        payer: &Keypair,
        target_token_account: &Address,
    ) -> Result<SimulationQuote> {
        let initial_target_balance = self.read_token_balance_from_svm(target_token_account)?;
        let recent_blockhash = self.svm.latest_blockhash();
        let message =
            Message::new_with_blockhash(instructions, Some(&payer.pubkey()), &recent_blockhash);
        let transaction =
            VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])
                .map_err(|err| anyhow!("failed to build simulation transaction: {err}"))?;

        let sim_result = self
            .svm
            .simulate_transaction(transaction)
            .map_err(|err| anyhow!("on-chain simulation failed: {err:?}"))?;
        let final_target_balance =
            resolve_post_simulation_balance(target_token_account, &sim_result.post_accounts)
                .or_else(|| {
                    self.svm
                        .get_account(target_token_account)
                        .map(|account| account.into())
                })
                .map(|account| unpack_token_account_balance(&account))
                .transpose()?
                .unwrap_or(initial_target_balance);

        Ok(SimulationQuote {
            amount_out: final_target_balance.saturating_sub(initial_target_balance),
            final_target_balance,
        })
    }

    fn read_token_balance_from_svm(&self, token_account: &Address) -> Result<u64> {
        let account = self
            .svm
            .get_account(token_account)
            .ok_or_else(|| anyhow!("target token account not found in SVM"))?;
        unpack_token_account_balance(&account)
    }
}

fn resolve_post_simulation_balance(
    target_token_account: &Address,
    post_accounts: &[(Address, AccountSharedData)],
) -> Option<Account> {
    post_accounts
        .iter()
        .find(|(address, _)| address == target_token_account)
        .map(|(_, account): &(Address, AccountSharedData)| account.clone().into())
}

fn unpack_token_account_balance(account: &Account) -> Result<u64> {
    let token_state = SplTokenAccount::unpack(&account.data)
        .map_err(|_| anyhow!("failed to unpack SPL token account"))?;
    Ok(token_state.amount)
}

#[cfg(test)]
mod tests {
    use super::{QuoteSimulator, resolve_post_simulation_balance, unpack_token_account_balance};
    use solana_account::{Account, AccountSharedData};
    use solana_address::Address;
    use solana_keypair::Keypair;
    use solana_program_option::COption;
    use solana_program_pack::Pack;
    use solana_signer::Signer;
    use spl_token_interface::state::{Account as SplTokenAccount, AccountState};

    fn token_account(amount: u64, mint: Address, owner: Address) -> Account {
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
        SplTokenAccount::pack(token_account, &mut data).unwrap();

        Account {
            lamports: 1_000_000,
            data,
            owner: Address::from([3u8; 32]),
            executable: false,
            rent_epoch: 0,
        }
    }

    #[test]
    fn resolves_balance_from_post_accounts() {
        let target = Address::from([9u8; 32]);
        let account = token_account(250, Address::from([1u8; 32]), Address::from([2u8; 32]));
        let post_accounts = vec![(target, AccountSharedData::from(account.clone()))];

        let resolved = resolve_post_simulation_balance(&target, &post_accounts).unwrap();

        assert_eq!(unpack_token_account_balance(&resolved).unwrap(), 250);
    }

    #[test]
    fn reports_zero_amount_out_when_target_balance_does_not_change() {
        let payer = Keypair::new();
        let target = Address::from([9u8; 32]);
        let mint = Address::from([1u8; 32]);
        let owner = Address::from([2u8; 32]);
        let mut simulator = QuoteSimulator::new();

        simulator.airdrop(&payer.pubkey(), 1_000_000).unwrap();
        simulator
            .load_accounts(vec![(target, token_account(500, mint, owner))])
            .unwrap();

        let result = simulator
            .simulate_transaction(&[], &payer, &target)
            .unwrap();

        assert_eq!(result.amount_out, 0);
        assert_eq!(result.final_target_balance, 500);
    }
}
