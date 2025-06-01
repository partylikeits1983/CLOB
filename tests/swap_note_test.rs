use std::time::Instant;

use miden_client::{
    ClientError, Felt,
    asset::FungibleAsset,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::NoteType,
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionRequestBuilder},
};

use std::sync::Arc;

use miden_crypto::rand::FeltRng;

use miden_clob::{
    common::{
        compute_partial_swapp, create_order, create_p2id_note, create_partial_swap_note,
        delete_keystore_and_store, get_p2id_serial_num, get_swapp_note, instantiate_client,
        setup_accounts_and_faucets, try_match_swapp_notes, wait_for_note,
    },
    create_order_simple,
};

#[tokio::test]
async fn swap_note_partial_consume_public_test() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

    // Initialize client
    let endpoint = Endpoint::localhost();

    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Create SWAPP note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    // offered asset amount
    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    // requested asset amount
    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    let swapp_note_id = swapp_note.id();

    // Time from after SWAPP creation
    let start_time = Instant::now();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 2: Partial Consume SWAPP note
    // -------------------------------------------------------------------------
    let fill_amount_bob = 25;
    let (_amount_a_1, new_amount_a, new_amount_b) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    // leftover portion of Alice’s original order
    let swapp_note_1 = create_partial_swap_note(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_amount_a)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_amount_b)
            .unwrap()
            .into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // P2ID note for Bob’s partial fill going to Alice
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);

    let p2id_note = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    client.sync_state().await?;

    // pass in amount to fill via note args
    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), Some(consume_amount_note_args))])
        .with_expected_output_notes(vec![p2id_note, swapp_note_1])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());

    let _ = client.submit_transaction(tx_result).await;

    // Stop timing
    let duration = start_time.elapsed();
    println!("SWAPP note partially filled");
    println!("Time from SWAPP creation to partial fill: {:?}", duration);

    Ok(())
}

#[tokio::test]
async fn fill_counter_party_swap_notes() -> Result<(), ClientError> {
    delete_keystore_and_store().await;

    let endpoint = Endpoint::localhost();
    let mut client = instantiate_client(endpoint).await?;
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    // -------------------------------------------------------------------------
    // STEP 1: Create Accounts (Alice, Bob, Matcher Account)
    // -------------------------------------------------------------------------
    // Setup accounts and balances
    let balances = vec![
        vec![100, 0],   // For account[0] => Alice
        vec![0, 100],   // For account[0] => Bob
        vec![100, 100], // For account[0] => matcher
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let matcher_account = accounts[2].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 2: Create the SWAP Notes
    // -------------------------------------------------------------------------
    let swap_note_1_asset_a = FungibleAsset::new(faucet_a.id(), 100).unwrap();
    let swap_note_1_asset_b = FungibleAsset::new(faucet_b.id(), 100).unwrap();
    let swap_note_1_serial_num = client.rng().draw_word();
    let swap_note_1 = create_partial_swap_note(
        alice_account.id(),         // creator of the order
        alice_account.id(),         // last account to "fill the order"
        swap_note_1_asset_a.into(), // offered asset (selling)
        swap_note_1_asset_b.into(), // requested asset (buying)
        swap_note_1_serial_num,     // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    let swap_note_2_asset_a = FungibleAsset::new(faucet_a.id(), 50).unwrap();
    let swap_note_2_asset_b = FungibleAsset::new(faucet_b.id(), 50).unwrap();
    let swap_note_2_serial_num = client.rng().draw_word();
    let swap_note_2 = create_partial_swap_note(
        bob_account.id(),
        bob_account.id(),
        swap_note_2_asset_b.into(),
        swap_note_2_asset_a.into(),
        swap_note_2_serial_num,
        0,
    )
    .unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swap_note_1.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swap_note_2.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(bob_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    // -------------------------------------------------------------------------
    // STEP 3: Computing output notes if SWAP notes are matched
    // -------------------------------------------------------------------------
    let p2id_serial_num_1 = get_p2id_serial_num(swap_note_1.serial_num(), 1);
    let p2id_1 = create_p2id_note(
        matcher_account.id(),             // sender
        alice_account.id(),               // account id to receive the asset
        vec![swap_note_2_asset_b.into()], // asset to send
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1, // p2id serial number for SWAPP note
    )
    .unwrap();

    let p2id_serial_num_2 = get_p2id_serial_num(swap_note_2.serial_num(), 1);
    let p2id_2 = create_p2id_note(
        matcher_account.id(),
        bob_account.id(),
        vec![swap_note_2_asset_a.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_2,
    )
    .unwrap();

    // Computing the remainder of the SWAP note that isn't completely filled. These are the remaining assets (offered, requested)
    let swap_note_3_asset_a = FungibleAsset::new(faucet_a.id(), 50).unwrap();
    let swap_note_3_asset_b = FungibleAsset::new(faucet_b.id(), 50).unwrap();

    // compute the new serial number ( previous serial number + 1)
    let swap_serial_num_3 = [
        swap_note_1_serial_num[0],
        swap_note_1_serial_num[1],
        swap_note_1_serial_num[2],
        Felt::new(swap_note_1_serial_num[3].as_int() + 1),
    ];

    // increment the swap count
    let swap_count_3 = swap_note_1.inputs().values()[8].as_int() + 1;

    let swap_note_3 = create_partial_swap_note(
        alice_account.id(),         // initial order creator
        matcher_account.id(), // matcher account is the last account to interact with the SWAP note
        swap_note_3_asset_a.into(), // remaining offered
        swap_note_3_asset_b.into(), // remaining requested
        swap_serial_num_3,    // new serial number
        swap_count_3,         // new swap count
    )
    .unwrap();

    // These are the "note args"
    // This is how to specify how much of the "requested" asset to give to the order
    let swap_note_1_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(50), // Giving 50 of requested asset (in this case this is faucet B asset)
    ];

    let swap_note_2_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(50), // Giving 50 of requested asset (in this case this is faucet A asset)
    ];

    println!("waiting");
    wait_for_note(&mut client, &matcher_account, &swap_note_1)
        .await
        .unwrap();
    wait_for_note(&mut client, &matcher_account, &swap_note_2)
        .await
        .unwrap();

    // ---------------------------------------------------------------------------------
    // STEP 3: Consume both SWAP notes in a single TX by the matcher & output p2id notes
    // ---------------------------------------------------------------------------------
    // Combined Transaction
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (swap_note_1.id(), Some(swap_note_1_note_args)), // note that isn't filled compltely
            (swap_note_2.id(), Some(swap_note_2_note_args)), // note that is filled completely
        ])
        // these are the outputted notes
        .with_expected_output_notes(vec![p2id_2.clone(), p2id_1.clone(), swap_note_3.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(matcher_account.id(), consume_custom_req)
        .await
        .unwrap();

    let _ = client.submit_transaction(tx_result).await;

    client.sync_state().await.unwrap();

    let binding = client
        .get_account(matcher_account.id())
        .await
        .unwrap()
        .unwrap();
    let matcher_account = binding.account();

    println!(
        "matcher account bal: A: {:?} B: {:?}",
        matcher_account.clone().vault().get_balance(faucet_a.id()),
        matcher_account.vault().get_balance(faucet_b.id())
    );

    Ok(())
}

#[tokio::test]
async fn swap_note_partial_consume_public_test_matched() -> Result<(), ClientError> {
    // ────────────────────────────────────────────────────────────
    // 0.  Reset the store and initialize the client
    // ────────────────────────────────────────────────────────────
    delete_keystore_and_store().await;

    let endpoint = Endpoint::localhost();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("[chain-sync] latest block = {}", sync_summary.block_num);

    // ────────────────────────────────────────────────────────────
    // 1.  Provision three accounts + two test faucets
    // ────────────────────────────────────────────────────────────
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 100],       // trader-1
        vec![100, 100],       // trader-2
        vec![10_000, 10_000], // matcher
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    let trader_1 = accounts[0].clone();
    let trader_2 = accounts[1].clone();
    let matcher = accounts[2].clone();

    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    println!("faucet: {:?}", faucet_a.id().to_hex());
    println!("faucet: {:?}", faucet_b.id().to_hex());
    println!("trader_1: {:?}", trader_1.id().to_hex());
    println!("trader_2: {:?}", trader_2.id().to_hex());
    // ────────────────────────────────────────────────────────────
    // 2.  Trader-1: “sell 50 A for 50 B” (maker)
    // ────────────────────────────────────────────────────────────
    let swap_note_1 = create_order(
        &mut client,
        trader_1.id(),
        FungibleAsset::new(faucet_a.id(), 100).unwrap().into(), // offered
        FungibleAsset::new(faucet_b.id(), 100).unwrap().into(), // wanted
    )
    .await
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 3.  Trader-2: “sell 25 B for 25 A” (taker)
    // ────────────────────────────────────────────────────────────
    let swap_note_2 = create_order(
        &mut client,
        trader_2.id(),
        FungibleAsset::new(faucet_b.id(), 50).unwrap().into(), // offered
        FungibleAsset::new(faucet_a.id(), 50).unwrap().into(), // wanted
    )
    .await
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 4.  Wait until the two SWAPP notes are visible on-chain
    // ────────────────────────────────────────────────────────────
    let _ = get_swapp_note(&mut client, swap_note_1.metadata().tag(), swap_note_1.id()).await;
    let _ = get_swapp_note(&mut client, swap_note_2.metadata().tag(), swap_note_2.id()).await;

    // ────────────────────────────────────────────────────────────
    // 5.  Off-chain matcher tries to cross the two orders
    // ────────────────────────────────────────────────────────────
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher.id())
        .unwrap()
        .expect("orders did not cross – test set-up is wrong");

    // ────────────────────────────────────────────────────────────
    // 6.  Build the single consume-transaction
    // ────────────────────────────────────────────────────────────
    // Expected outputs = 2 P2ID notes (+ optional residual SWAPP note)
    let mut expected_outputs = vec![
        swap_data.p2id_from_1_to_2.clone(),
        swap_data.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push(note.clone());
    }
    expected_outputs.sort_by_key(|n| n.commitment());

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (swap_note_1.id(), Some(swap_data.note1_args)), // maker’s SWAPP note
            (swap_note_2.id(), Some(swap_data.note2_args)), // taker’s SWAPP note
        ])
        .with_expected_output_notes(expected_outputs)
        .build()
        .unwrap();

    // ────────────────────────────────────────────────────────────
    // 7.  Submit and confirm
    // ────────────────────────────────────────────────────────────
    let tx = client
        .new_transaction(matcher.id(), consume_req)
        .await
        .unwrap();
    client.submit_transaction(tx).await.unwrap();
    client.sync_state().await.unwrap();

    // (optional) quick sanity print of post-trade balances
    let binding = client.get_account(matcher.id()).await.unwrap().unwrap();
    let matcher_acct = binding.account();
    println!(
        "[matcher] balances ⇒ A: {:?}  B: {:?}",
        matcher_acct.vault().get_balance(faucet_a.id()),
        matcher_acct.vault().get_balance(faucet_b.id())
    );

    Ok(())
}

#[tokio::test]
async fn swap_note_edge_case_test() -> Result<(), ClientError> {
    // ────────────────────────────────────────────────────────────
    // 0.  Reset the store and initialize the client
    // ────────────────────────────────────────────────────────────
    delete_keystore_and_store().await;

    let endpoint = Endpoint::localhost();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("[chain-sync] latest block = {}", sync_summary.block_num);

    // ────────────────────────────────────────────────────────────
    // 1.  Provision three accounts + two test faucets
    // ────────────────────────────────────────────────────────────
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100_000_000, 100_000_000], // trader-1
        vec![100_000_000, 100_000_000], // trader-2
        vec![100_000_000, 100_000_000], // matcher
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    let trader_1 = accounts[0].clone();
    let trader_2 = accounts[1].clone();
    let matcher = accounts[2].clone();

    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    println!("faucet: {:?}", faucet_a.id().to_hex());
    println!("faucet: {:?}", faucet_b.id().to_hex());
    println!("trader_1: {:?}", trader_1.id().to_hex());
    println!("trader_2: {:?}", trader_2.id().to_hex());
    // ────────────────────────────────────────────────────────────
    // 2.  Trader-1: “sell 50 A for 50 B” (maker)
    // ────────────────────────────────────────────────────────────
    let swap_note_1 = create_order_simple(
        &mut client,
        trader_1.id(),
        FungibleAsset::new(faucet_a.id(), 1815515).unwrap().into(),
        FungibleAsset::new(faucet_b.id(), 689).unwrap().into(),
    )
    .await
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 3.  Trader-2: “sell 25 B for 25 A” (taker)
    // ────────────────────────────────────────────────────────────
    let swap_note_2 = create_order_simple(
        &mut client,
        trader_2.id(),
        FungibleAsset::new(faucet_b.id(), 522).unwrap().into(), // offered (use exact amount from working match)
        FungibleAsset::new(faucet_a.id(), 1368162).unwrap().into(), // wanted (use exact amount from working match)
    )
    .await
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 4.  Wait until the two SWAPP notes are visible on-chain
    // ────────────────────────────────────────────────────────────
    let _ = get_swapp_note(&mut client, swap_note_1.metadata().tag(), swap_note_1.id()).await;
    let _ = get_swapp_note(&mut client, swap_note_2.metadata().tag(), swap_note_2.id()).await;

    // ────────────────────────────────────────────────────────────
    // 5.  Off-chain matcher tries to cross the two orders
    // ────────────────────────────────────────────────────────────
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher.id())
        .unwrap()
        .expect("orders did not cross – test set-up is wrong");

    println!("note args1: {:?}", swap_data.note1_args);
    println!("note args2: {:?}", swap_data.note2_args);

    // ────────────────────────────────────────────────────────────
    // 6.  Build the single consume-transaction
    // ────────────────────────────────────────────────────────────
    // Expected outputs = 2 P2ID notes (+ optional residual SWAPP note)
    let mut expected_outputs = vec![
        swap_data.p2id_from_1_to_2.clone(),
        swap_data.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push(note.clone());
    }
    expected_outputs.sort_by_key(|n| n.commitment());

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([
            (swap_note_1.id(), Some(swap_data.note1_args)), // maker’s SWAPP note
            (swap_note_2.id(), Some(swap_data.note2_args)), // taker’s SWAPP note
        ])
        .with_expected_output_notes(expected_outputs)
        .build()
        .unwrap();

    // ────────────────────────────────────────────────────────────
    // 7.  Submit and confirm
    // ────────────────────────────────────────────────────────────
    let tx = client
        .new_transaction(matcher.id(), consume_req)
        .await
        .unwrap();
    client.submit_transaction(tx).await.unwrap();
    client.sync_state().await.unwrap();

    // (optional) quick sanity print of post-trade balances
    let binding = client.get_account(matcher.id()).await.unwrap().unwrap();
    let matcher_acct = binding.account();
    println!(
        "[matcher] balances ⇒ A: {:?}  B: {:?}",
        matcher_acct.vault().get_balance(faucet_a.id()),
        matcher_acct.vault().get_balance(faucet_b.id())
    );

    Ok(())
}

#[tokio::test]
async fn swap_note_reclaim_public_test() -> Result<(), ClientError> {
    // Reset the store and initialize the client.
    delete_keystore_and_store().await;

    // Initialize client
    let _endpoint = Endpoint::new(
        "https".to_string(),
        "rpc.testnet.miden.io".to_string(),
        Some(443),
    );
    let endpoint = Endpoint::localhost();

    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // For account[0] => Alice
        vec![0, 100], // For account[1] => Bob
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 2, 2, balances).await?;

    // rename for clarity
    let alice_account = accounts[0].clone();
    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Create SWAPP note
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note");

    // offered asset amount
    let amount_a = 50;
    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();

    // requested asset amount
    let amount_b = 50;
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let swapp_tag = swapp_note.metadata().tag();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await?;

    let swapp_note_id = swapp_note.id();

    // Time from after SWAPP creation
    let start_time = Instant::now();

    let _ = get_swapp_note(&mut client, swapp_tag, swapp_note_id).await;

    // -------------------------------------------------------------------------
    // STEP 2: Reclaim SWAPP note
    // -------------------------------------------------------------------------

    println!(
        "alice account id: {:?} {:?}",
        alice_account.id().prefix(),
        alice_account.id().suffix()
    );

    let consume_custom_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(swapp_note.id(), None)])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(alice_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Consumed Note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());

    let _ = client.submit_transaction(tx_result).await;

    // Stop timing
    let duration = start_time.elapsed();
    println!("SWAPP note partially filled");
    println!("Time from SWAPP creation to partial fill: {:?}", duration);

    Ok(())
}

#[tokio::test]
async fn partial_swap_chain_public_optimistic_benchmark() -> Result<(), ClientError> {
    delete_keystore_and_store().await;

    // Initialize client
    let endpoint = Endpoint::localhost();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api)
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let sync_summary = client.sync_state().await.unwrap();
    println!("Latest block: {}", sync_summary.block_num);

    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    let balances = vec![
        vec![100, 0], // Alice
        vec![0, 100], // Bob
        vec![0, 100], // Charlie
    ];

    // This returns `(Vec<Account>, Vec<Account>)` => (accounts, faucets)
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;

    let alice_account = accounts[0].clone();
    let bob_account = accounts[1].clone();
    let charlie_account = accounts[2].clone();

    let faucet_a = faucets[0].clone();
    let faucet_b = faucets[1].clone();

    // -------------------------------------------------------------------------
    // STEP 1: Alice creates a SWAPP note: 50 A → 50 B
    // -------------------------------------------------------------------------
    println!("\n[STEP 3] Create SWAPP note for Alice: 50 A -> 50 B");

    let amount_a = 50; // offered A
    let amount_b = 50; // requested B

    let asset_a = FungibleAsset::new(faucet_a.id(), amount_a).unwrap();
    let asset_b = FungibleAsset::new(faucet_b.id(), amount_b).unwrap();

    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        alice_account.id(), // “Seller” side
        alice_account.id(),
        asset_a.into(),
        asset_b.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_req)
        .await
        .unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    client.submit_transaction(tx_result).await?;
    client.sync_state().await?;

    // -------------------------------------------------------------------------
    // STEP 2: Bob partially consumes swapp_note (optimistically)
    // -------------------------------------------------------------------------
    println!("\n[STEP 4] Bob partially fills SWAPP note with 25 B");
    let start_time_bob = Instant::now(); // measure Bob's fill time

    let fill_amount_bob: u64 = 25;

    // Bob has 25 B => fill ratio => 25 B in => 25 A out
    // leftover => 25 A, 25 B
    let (_amount_out_offered, new_offered_asset_amount, new_requested_asset_amount) =
        compute_partial_swapp(amount_a, amount_b, fill_amount_bob);

    let swap_serial_num_1 = [
        swap_serial_num[0],
        swap_serial_num[1],
        swap_serial_num[2],
        Felt::new(swap_serial_num[3].as_int() + 1),
    ];
    let swap_count_1 = swap_count + 1;

    // leftover portion -> swapp_note_1
    let swapp_note_1 = create_partial_swap_note(
        alice_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a.id(), new_offered_asset_amount)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_requested_asset_amount)
            .unwrap()
            .into(),
        swap_serial_num_1,
        swap_count_1,
    )
    .unwrap();

    // p2id note for Bob’s partial fill => 25 B in for Alice
    let p2id_note_asset_1 = FungibleAsset::new(faucet_b.id(), fill_amount_bob).unwrap();
    let p2id_serial_num_1 = get_p2id_serial_num(swap_serial_num, swap_count_1);
    let p2id_note_1 = create_p2id_note(
        bob_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_1.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_1,
    )
    .unwrap();

    // Pass Bob's partial fill args
    let consume_amount_note_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_bob),
    ];

    // Use optimistic consumption (no proof needed from note side)
    let consume_custom_req = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note, Some(consume_amount_note_args))])
        .with_expected_output_notes(vec![p2id_note_1.clone(), swapp_note_1.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(bob_account.id(), consume_custom_req)
        .await
        .unwrap();

    println!(
        "Bob consumed swapp_note Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    client.submit_transaction(tx_result).await?;

    let duration_bob = start_time_bob.elapsed();
    println!("SWAPP note partially filled by Bob");
    println!("Time for Bob’s partial fill: {duration_bob:?}");

    // -------------------------------------------------------------------------
    // STEP 3: Charlie partially consumes swapp_note_1 (optimistically)
    // -------------------------------------------------------------------------
    println!("\n[STEP 5] Charlie partially fills swapp_note_1 with 10 B");
    let start_time_charlie = Instant::now(); // measure Charlie's fill time

    let fill_amount_charlie = 10;

    // leftover from step 4 => leftover_a_1, leftover_b_1
    // Charlie has 10 B => partial fill => 10 A out, leftover => 15 A, 15 B
    let (_a_out_2, new_offered_asset_amount_1, new_requested_asset_amount_1) =
        compute_partial_swapp(
            new_offered_asset_amount,
            new_requested_asset_amount,
            fill_amount_charlie,
        );

    let swap_serial_num_2 = [
        swap_serial_num_1[0],
        swap_serial_num_1[1],
        swap_serial_num_1[2],
        Felt::new(swap_serial_num_1[3].as_int() + 1),
    ];
    let swap_count_2 = swap_count_1 + 1;

    let swapp_note_2 = create_partial_swap_note(
        alice_account.id(),
        charlie_account.id(),
        FungibleAsset::new(faucet_a.id(), new_offered_asset_amount_1)
            .unwrap()
            .into(),
        FungibleAsset::new(faucet_b.id(), new_requested_asset_amount_1)
            .unwrap()
            .into(),
        swap_serial_num_2,
        swap_count_2,
    )
    .unwrap();

    let p2id_note_asset_2 = FungibleAsset::new(faucet_b.id(), fill_amount_charlie).unwrap();
    let p2id_serial_num_2 = get_p2id_serial_num(swap_serial_num_1, swap_count_2);
    let p2id_note_2 = create_p2id_note(
        charlie_account.id(),
        alice_account.id(),
        vec![p2id_note_asset_2.into()],
        NoteType::Public,
        Felt::new(0),
        p2id_serial_num_2,
    )
    .unwrap();

    let consume_amount_note_args_charlie = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_amount_charlie),
    ];
    let consume_custom_req_charlie = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([(swapp_note_1, Some(consume_amount_note_args_charlie))])
        .with_expected_output_notes(vec![p2id_note_2.clone(), swapp_note_2.clone()])
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(charlie_account.id(), consume_custom_req_charlie)
        .await
        .unwrap();

    println!(
        "Charlie consumed swapp_note_1 Tx on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );
    println!("account delta: {:?}", tx_result.account_delta().vault());
    client.submit_transaction(tx_result).await?;

    let duration_charlie = start_time_charlie.elapsed();
    println!("SWAPP note partially filled by Charlie");
    println!("Time for Charlie’s partial fill: {duration_charlie:?}");

    println!(
        "SWAPP note leftover after Charlie’s partial fill => A: {}, B: {}",
        new_offered_asset_amount_1, new_requested_asset_amount_1
    );
    println!("Done with partial swap ephemeral chain test.");

    Ok(())
}

#[tokio::test]
async fn test_compute_partial_swapp() -> Result<(), ClientError> {
    let amount_b_in = 25;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        100,         // originally offered A
        50,          // originally requested B
        amount_b_in, // Bob fills 25 B
    );

    println!("==== test_compute_partial_swapp ====");
    println!("amount_a_1 (A out):          {}", amount_a_1);
    println!("amount_b_1 (B in):           {}", amount_b_in);
    println!("new_amount_a (A leftover):   {}", new_amount_a);
    println!("new_amount_b (B leftover):   {}", new_amount_b);

    assert_eq!(amount_a_1, 50);
    assert_eq!(amount_b_in, 25);
    assert_eq!(new_amount_a, 50);
    assert_eq!(new_amount_b, 25);

    let amount_b_in = 2500;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        5000,        // originally offered A
        5000,        // originally requested B
        amount_b_in, // Bob fills 2500 B
    );

    // 1. For a 1:1 ratio: 2500 B in => 2500 A out => leftover 2500 A / 2500 B
    assert_eq!(amount_a_1, 2500);
    assert_eq!(amount_b_in, 2500);
    assert_eq!(new_amount_a, 2500);
    assert_eq!(new_amount_b, 2500);

    Ok(())
}

#[tokio::test]
async fn test_compute_partial_swapp_edge_case() -> Result<(), ClientError> {
    let amount_b_in = 522;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        1368162,     // originally offered A
        522,         // originally requested B
        amount_b_in, // Bob fills 25 B
    );

    println!("==== test_compute_partial_swapp ====");
    println!("amount_a_1 (A out):          {}", amount_a_1);
    println!("amount_b_1 (B in):           {}", amount_b_in);
    println!("new_amount_a (A leftover):   {}", new_amount_a);
    println!("new_amount_b (B leftover):   {}", new_amount_b);

    Ok(())
}
