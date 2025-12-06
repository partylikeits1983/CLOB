use miden_assembly::Assembler;
use rand::{rngs::StdRng, RngCore};
use std::{env, sync::Arc};

use miden_client::{
    account::{
        component::{BasicFungibleFaucet, BasicWallet, RpoFalcon512},
        Account, AccountBuilder, AccountStorageMode, AccountType, StorageSlot,
    },
    asset::{FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    builder::ClientBuilder,
    crypto::SecretKey,
    keystore::FilesystemKeyStore,
    rpc::TonicRpcClient,
    transaction::TransactionRequestBuilder,
    Client, ClientError, Felt, Word,
};
use miden_lib::account::auth;
use miden_objects::account::AccountComponent;

pub async fn create_basic_account(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<(miden_client::account::Account, SecretKey), ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = SecretKey::with_rng(client.rng());
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Private)
        .with_auth_component(RpoFalcon512::new(key_pair.public_key().clone()))
        .with_component(BasicWallet);
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair.clone()))
        .unwrap();

    Ok((account, key_pair))
}

// TODO: Currently faucets are setup with `NoAuth` auth component
pub async fn create_basic_faucet(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = SecretKey::with_rng(client.rng());
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000_000_000);
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(auth::NoAuth)
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap());
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair))
        .unwrap();
    Ok(account)
}

/// Creates [num_accounts] accounts, [num_faucets] faucets, and mints the given [balances].
///
/// - `balances[a][f]`: how many tokens faucet `f` should mint for account `a`.
/// - Returns: a tuple of `(Vec<Account>, Vec<Account>)` i.e. (accounts, faucets).
pub async fn setup_accounts_and_faucets(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
    num_accounts: usize,
    num_faucets: usize,
    balances: Vec<Vec<u64>>,
) -> Result<(Vec<Account>, Vec<Account>), ClientError> {
    // ---------------------------------------------------------------------
    // 1)  Create basic accounts
    // ---------------------------------------------------------------------
    let mut accounts = Vec::with_capacity(num_accounts);
    for i in 0..num_accounts {
        let (account, _) = create_basic_account(client, keystore.clone()).await?;
        println!("Created Account #{i} ⇒ ID: {:?}", account.id().to_hex());
        accounts.push(account);
    }

    // ---------------------------------------------------------------------
    // 2)  Create basic faucets
    // ---------------------------------------------------------------------
    let mut faucets = Vec::with_capacity(num_faucets);
    for j in 0..num_faucets {
        let faucet = create_basic_faucet(client, keystore.clone()).await?;
        println!("Created Faucet #{j} ⇒ ID: {:?}", faucet.id().to_hex());
        faucets.push(faucet);
    }

    // Tell the client about the new accounts/faucets
    client.sync_state().await?;

    // ---------------------------------------------------------------------
    // 3)  Mint tokens
    // ---------------------------------------------------------------------
    // `minted_notes[i]` collects the notes minted **for** `accounts[i]`
    let mut minted_notes: Vec<Vec<miden_client::note::Note>> = vec![Vec::new(); num_accounts];

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
                    miden_client::note::NoteType::Public,
                    client.rng(),
                )
                .unwrap();

            let tx_exec = client.new_transaction(faucet.id(), tx_request).await?;
            client.submit_transaction(tx_exec.clone()).await?;

            // Remember the freshly-created note so we can consume it later
            let minted_note = match tx_exec.created_notes().get_note(0) {
                miden_client::transaction::OutputNote::Full(n) => n.clone(),
                _ => panic!("Expected OutputNote::Full, got something else"),
            };
            minted_notes[acct_idx].push(minted_note);
        }
    }

    // ---------------------------------------------------------------------
    // 4)  ONE wait-phase – ensure every account can now see all its notes
    // ---------------------------------------------------------------------
    for (acct_idx, account) in accounts.iter().enumerate() {
        let expected = minted_notes[acct_idx].len();
        if expected > 0 {
            crate::client::wait_for_notes(client, account, expected).await?;
        }
    }
    client.sync_state().await?;

    // ---------------------------------------------------------------------
    // 5)  Consume notes so the tokens live in the public vaults
    // ---------------------------------------------------------------------
    for (acct_idx, account) in accounts.iter().enumerate() {
        for note in &minted_notes[acct_idx] {
            let consume_req = TransactionRequestBuilder::new()
                .authenticated_input_notes([(note.id(), None)])
                .build()
                .unwrap();

            let tx_exec = client.new_transaction(account.id(), consume_req).await?;
            client.submit_transaction(tx_exec).await?;
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
        vec![StorageSlot::Value([
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
        ])],
    )
    .unwrap()
    .with_supports_all_types();

    // @dev this is bad that I need to get an anchor block to create a contract
    let endpoint: miden_client::rpc::Endpoint =
        miden_client::rpc::Endpoint::try_from(env::var("MIDEN_NODE_ENDPOINT").unwrap().as_str())
            .unwrap();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));
    let mut client = ClientBuilder::new()
        .rpc(rpc_api.clone())
        .filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let (counter_contract, counter_seed) = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(counter_component.clone())
        .with_component(BasicWallet)
        .build()
        .unwrap();

    Ok((counter_contract, counter_seed))
}
