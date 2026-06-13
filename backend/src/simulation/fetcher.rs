use anyhow::Result;
use solana_account::Account;
use solana_address::Address;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

pub struct AccountFetcher;

pub type FetchedAccounts = Vec<(Address, Account)>;

impl AccountFetcher {
    /// Fetches accounts in batches to respect RPC limits, ensuring no duplicates.
    pub fn fetch_accounts(rpc_client: &RpcClient, pubkeys: &[Pubkey]) -> Result<FetchedAccounts> {
        let mut unique_keys = pubkeys.to_vec();
        unique_keys.sort();
        unique_keys.dedup();

        let mut fetched_accounts = Vec::with_capacity(unique_keys.len());

        for chunk in unique_keys.chunks(100) {
            let accounts_data = rpc_client.get_multiple_accounts(chunk)?;

            for (pubkey, account_opt) in chunk.iter().zip(accounts_data.into_iter()) {
                if let Some(account) = account_opt {
                    fetched_accounts.push((convert_pubkey(pubkey), convert_account(account)));
                } else {
                    println!("Warning: Account {} not found on-chain", pubkey);
                }
            }
        }

        Ok(fetched_accounts)
    }
}

fn convert_pubkey(pubkey: &Pubkey) -> Address {
    Address::from(pubkey.to_bytes())
}

fn convert_account(account: Account) -> Account {
    account
}

#[cfg(test)]
mod tests {
    use super::{convert_account, convert_pubkey};
    use solana_account::Account;
    use solana_sdk::pubkey::Pubkey;

    #[test]
    fn converts_sdk_types_to_litesvm_types() {
        let owner = Pubkey::new_from_array([7u8; 32]);
        let pubkey = Pubkey::new_from_array([9u8; 32]);
        let account = Account {
            lamports: 42,
            data: vec![1, 2, 3],
            owner: super::convert_pubkey(&owner),
            executable: false,
            rent_epoch: 5,
        };

        let converted_pubkey = convert_pubkey(&pubkey);
        let converted_account = convert_account(account);

        assert_eq!(converted_pubkey.to_bytes(), pubkey.to_bytes());
        assert_eq!(converted_account.lamports, 42);
        assert_eq!(converted_account.owner.to_bytes(), owner.to_bytes());
        assert_eq!(converted_account.data, vec![1, 2, 3]);
    }
}
