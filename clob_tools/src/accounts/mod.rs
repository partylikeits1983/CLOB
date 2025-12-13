use rand::{rngs::StdRng, RngCore};
use std::{env, sync::Arc};

use miden_client::{
    account::{
        component::{BasicFungibleFaucet, BasicWallet},
        Account, AccountStorageMode, AccountType, StorageSlot,
    },
    asset::{FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{NoteType},
    rpc::GrpcClient,
    transaction::TransactionRequestBuilder,
    Client, ClientError, Felt, Word,
};
use miden_lib::account::auth::AuthRpoFalcon512;
use miden_objects::{
    account::{AccountBuilder, AccountComponent},
    assembly::Assembler,
};

pub async fn create_basic_account(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    keystore: &Arc<FilesystemKeyStore<StdRng>>,
) -> Result<Account, ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = AuthSecretKey::new_rpo_falcon512();

    let account = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthRpoFalcon512::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet)
        .build()
        .unwrap();

    client.add_account(&account, false).await?;
    keystore.add_key(&key_pair).unwrap();

    Ok(account)
}

pub async fn create_basic_faucet(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    keystore: &Arc<FilesystemKeyStore<StdRng>>,
) -> Result<Account, ClientError> {
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = AuthSecretKey::new_rpo_falcon512();
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(100_000_000_000);

    let account = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthRpoFalcon512::new(key_pair.public_key().to_commitment()))
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap())
        .build()
        .unwrap();

    client.add_account(&account, false).await?;
    keystore.add_key(&key_pair).unwrap();

    Ok(account)
}

/// Creates [num_accounts] accounts, [num_faucets] faucets, and mints the given [balances].
///
/// - `balances[a][f]`: how many tokens faucet `f` should mint for account `a`.
/// - Returns: a tuple of `(Vec<Account>, Vec<Account>)` i.e. (accounts, faucets).
pub async fn setup_accounts_and_faucets(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    keystore: &Arc<FilesystemKeyStore<StdRng>>,
    num_accounts: usize,
    num_faucets: usize,
    balances: Vec<Vec<u64>>,
) -> Result<(Vec<Account>, Vec<Account>), ClientError> {
    use crate::client::{wait_for_notes};

    // ---------------------------------------------------------------------
    // 1)  Create basic accounts
    // ---------------------------------------------------------------------
    let mut accounts = Vec::with_capacity(num_accounts);
    for i in 0..num_accounts {
        let account = create_basic_account(client, keystore).await?;
        println!("Created Account #{i} ⇒ ID: {:?}", account.id().to_hex());
        accounts.push(account);
    }

    // ---------------------------------------------------------------------
    // 2)  Create basic faucets
    // ---------------------------------------------------------------------
    let mut faucets = Vec::with_capacity(num_faucets);
    for j in 0..num_faucets {
        let faucet = create_basic_faucet(client, keystore).await?;
        println!("Created Faucet #{j} ⇒ ID: {:?}", faucet.id().to_hex());
        faucets.push(faucet);
    }

    // Tell the client about the new accounts/faucets
    client.sync_state().await?;

    // ---------------------------------------------------------------------
    // 3)  Mint tokens and wait for each note before consuming
    // ---------------------------------------------------------------------
    for (acct_idx, account) in accounts.iter().enumerate() {
        for (faucet_idx, faucet) in faucets.iter().enumerate() {
            let amount = balances[acct_idx][faucet_idx];
            if amount == 0 {
                continue;
            }

            println!("Minting {amount} tokens from Faucet #{faucet_idx} to Account #{acct_idx}");

            // Build & submit the mint transaction
            let asset = FungibleAsset::new(faucet.id(), amount).unwrap();
            let tx_request = TransactionRequestBuilder::new()
                .build_mint_fungible_asset(
                    asset,
                    account.id(),
                    NoteType::Public,
                    client.rng(),
                )
                .unwrap();

            let tx_id = client
                .submit_new_transaction(faucet.id(), tx_request)
                .await?;
            println!("Minted tokens. TX: {:?}", tx_id);

            // Wait for the minted note to be available
            wait_for_notes(client, account, 1).await?;

            // Get and consume the minted note
            let consumable_notes = client.get_consumable_notes(Some(account.id())).await?;
            if let Some((note_record, _)) = consumable_notes.first() {
                let consume_req = TransactionRequestBuilder::new()
                    .build_consume_notes(vec![note_record.id()])
                    .unwrap();

                let tx_id = client
                    .submit_new_transaction(account.id(), consume_req)
                    .await?;
                println!("Consumed note. TX: {:?}", tx_id);
            }
        }
    }

    client.sync_state().await?;
    Ok((accounts, faucets))
}

// Contract builder helper function
pub async fn create_public_immutable_contract(
    account_code: &String,
) -> Result<(Account, Word), ClientError> {
    let assembler: Assembler =
        miden_client::transaction::TransactionKernel::assembler().with_debug_mode(true);

    let counter_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![StorageSlot::Value(
            [Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(0)].into(),
        )],
    )
    .unwrap()
    .with_supports_all_types();

    // @dev this is bad that I need to get an anchor block to create a contract
    let endpoint: miden_client::rpc::Endpoint =
        miden_client::rpc::Endpoint::try_from(env::var("MIDEN_NODE_ENDPOINT").unwrap().as_str())
            .unwrap();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(GrpcClient::new(&endpoint, timeout_ms));
    let keystore_path = std::path::PathBuf::from("./keystore");
    let keystore = Arc::new(FilesystemKeyStore::<StdRng>::new(keystore_path).unwrap());

    let mut client = ClientBuilder::new()
        .rpc(rpc_api.clone())
        .authenticator(keystore.clone())
        .in_debug_mode(true.into())
        .build()
        .await?;

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let counter_contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(counter_component.clone())
        .with_component(BasicWallet)
        .build()
        .unwrap();

    let seed_bytes: [u8; 4] = init_seed[..4].try_into().unwrap();
    Ok((counter_contract, Word::from(&seed_bytes)))
}
