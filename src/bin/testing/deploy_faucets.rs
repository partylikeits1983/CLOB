use miden_lib::account::auth;
use rand::RngCore;
use std::sync::Arc;
use tokio::time::Duration;

use miden_client::{
    account::{
        component::{BasicFungibleFaucet, BasicWallet, RpoFalcon512},
        AccountBuilder, AccountId, AccountStorageMode, AccountType,
    },
    asset::{FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    builder::ClientBuilder,
    crypto::SecretKey,
    keystore::FilesystemKeyStore,
    note::{create_p2id_note, NoteType},
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, PaymentNoteDescription, TransactionRequestBuilder},
    ClientError, Felt,
};
use miden_objects::account::{AccountIdVersion, NetworkId};

#[tokio::main]
async fn main() -> Result<(), ClientError> {
    // Initialize client & keystore
    let endpoint = Endpoint::testnet();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .rpc(rpc_api)
        .filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore: FilesystemKeyStore<rand::prelude::StdRng> =
        FilesystemKeyStore::new("./keystore".into()).unwrap();

    //------------------------------------------------------------
    // STEP 1: Create a basic wallet for Alice
    //------------------------------------------------------------
    println!("\n[STEP 1] Creating a new account for Alice");

    // Account seed
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = SecretKey::with_rng(client.rng());

    // Build the account
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(RpoFalcon512::new(key_pair.public_key()))
        .with_component(BasicWallet);

    let (alice_account, seed) = builder.build().unwrap();

    // Add the account to the client
    client
        .add_account(&alice_account, Some(seed), false)
        .await?;

    // Add the key pair to the keystore
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair))
        .unwrap();

    println!(
        "Alice's account ID: {:?}",
        alice_account.id().to_bech32(NetworkId::Testnet)
    );

    //------------------------------------------------------------
    // STEP 2: Deploy a fungible faucet
    //------------------------------------------------------------
    println!("\n[STEP 2] Deploying a new fungible faucet.");

    // Faucet seed
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    // Faucet parameters
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000);

    // Generate key pair
    let key_pair = SecretKey::with_rng(client.rng());

    // Build the account
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(auth::NoAuth)
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap());

    let (faucet_account, seed) = builder.build().unwrap();

    // Add the faucet to the client
    client
        .add_account(&faucet_account, Some(seed), false)
        .await?;

    // Add the key pair to the keystore
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair))
        .unwrap();

    println!(
        "Faucet account ID: {:?}",
        faucet_account.id().to_bech32(NetworkId::Testnet)
    );

    // Resync to show newly deployed faucet
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    //------------------------------------------------------------
    // STEP 3: Mint 5 notes of 100 tokens for Alice
    //------------------------------------------------------------
    println!("\n[STEP 3] Minting 5 notes of 100 tokens each for Alice.");

    let amount: u64 = 100;
    let fungible_asset = FungibleAsset::new(faucet_account.id(), amount).unwrap();

    for i in 1..=5 {
        let transaction_request = TransactionRequestBuilder::new()
            .build_mint_fungible_asset(
                fungible_asset,
                alice_account.id(),
                NoteType::Public,
                client.rng(),
            )
            .unwrap();

        println!("tx request built");

        let tx_execution_result = client
            .new_transaction(faucet_account.id(), transaction_request)
            .await?;
        client.submit_transaction(tx_execution_result).await?;
        println!("Minted note #{} of {} tokens for Alice.", i, amount);
    }
    println!("All 5 notes minted for Alice successfully!");

    // Re-sync so minted notes become visible
    client.sync_state().await?;

    //------------------------------------------------------------
    // STEP 4: Alice consumes all her notes
    //------------------------------------------------------------
    println!("\n[STEP 4] Alice will now consume all of her notes to consolidate them.");

    // Consume all minted notes in a single transaction
    loop {
        // Resync to get the latest data
        client.sync_state().await?;

        let consumable_notes = client
            .get_consumable_notes(Some(alice_account.id()))
            .await?;
        let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();

        if list_of_note_ids.len() == 5 {
            println!("Found 5 consumable notes for Alice. Consuming them now...");
            let transaction_request = TransactionRequestBuilder::new()
                .build_consume_notes(list_of_note_ids)
                .unwrap();
            let tx_execution_result = client
                .new_transaction(alice_account.id(), transaction_request)
                .await?;

            client.submit_transaction(tx_execution_result).await?;
            println!("All of Alice's notes consumed successfully.");
            break;
        } else {
            println!(
                "Currently, Alice has {} consumable notes. Waiting...",
                list_of_note_ids.len()
            );
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    //------------------------------------------------------------
    // STEP 5: Alice sends 5 notes of 50 tokens to 5 users
    //------------------------------------------------------------
    println!("\n[STEP 5] Alice sends 5 notes of 50 tokens each to 5 different users.");

    // Send 50 tokens to 4 accounts in one transaction
    println!("Creating multiple P2ID notes for 4 target accounts in one transaction...");
    let mut p2id_notes = vec![];

    // Creating 4 P2ID notes to 4 'dummy' AccountIds
    for _ in 1..=4 {
        let init_seed: [u8; 15] = {
            let mut init_seed = [0_u8; 15];
            client.rng().fill_bytes(&mut init_seed);
            init_seed
        };
        let target_account_id = AccountId::dummy(
            init_seed,
            AccountIdVersion::Version0,
            AccountType::RegularAccountUpdatableCode,
            AccountStorageMode::Public,
        );

        let send_amount = 50;
        let fungible_asset = FungibleAsset::new(faucet_account.id(), send_amount).unwrap();

        let p2id_note = create_p2id_note(
            alice_account.id(),
            target_account_id,
            vec![fungible_asset.into()],
            NoteType::Public,
            Felt::new(0),
            client.rng(),
        )?;
        p2id_notes.push(p2id_note);
    }

    // Specifying output notes and creating a tx request to create them
    let output_notes: Vec<OutputNote> = p2id_notes.into_iter().map(OutputNote::Full).collect();
    let transaction_request = TransactionRequestBuilder::new()
        .own_output_notes(output_notes)
        .build()
        .unwrap();

    let tx_execution_result = client
        .new_transaction(alice_account.id(), transaction_request)
        .await?;

    // Submitting the transaction
    client.submit_transaction(tx_execution_result).await?;
    println!("Submitted a transaction with 4 P2ID notes.");

    println!("Submitting one more single P2ID transaction...");
    let init_seed: [u8; 15] = {
        let mut init_seed = [0_u8; 15];
        client.rng().fill_bytes(&mut init_seed);
        init_seed
    };
    let target_account_id = AccountId::dummy(
        init_seed,
        AccountIdVersion::Version0,
        AccountType::RegularAccountUpdatableCode,
        AccountStorageMode::Public,
    );

    let send_amount = 50;
    let fungible_asset = FungibleAsset::new(faucet_account.id(), send_amount).unwrap();

    let payment_transaction = PaymentNoteDescription::new(
        vec![fungible_asset.into()],
        alice_account.id(),
        target_account_id,
    );

    let transaction_request = TransactionRequestBuilder::new()
        .build_pay_to_id(
            payment_transaction,
            NoteType::Public, // note type
            client.rng(),     // rng
        )
        .unwrap();
    let tx_execution_result = client
        .new_transaction(alice_account.id(), transaction_request)
        .await?;

    client.submit_transaction(tx_execution_result).await?;

    println!("\nAll steps completed successfully!");
    println!("Alice created a wallet, a faucet was deployed,");
    println!("5 notes of 100 tokens were minted to Alice, those notes were consumed,");
    println!("and then Alice sent 5 separate 50-token notes to 5 different users.");

    Ok(())
}
