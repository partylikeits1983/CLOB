use std::time::Instant;

use miden_client::{
    ClientError, Felt,
    asset::FungibleAsset,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{Note, NoteType},
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionRequestBuilder},
};

use std::sync::Arc;

use miden_clob::common::{
    compute_partial_swapp, create_p2id_note, create_partial_swap_note, creator_of,
    decompose_swapp_note, delete_keystore_and_store, generate_depth_chart, get_p2id_serial_num,
    get_swapp_note, instantiate_client, price_to_swap_note, setup_accounts_and_faucets,
    try_match_swapp_notes, wait_for_note,
};
use miden_crypto::rand::FeltRng;

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
async fn fill_counter_party_swap_notes_manual() -> Result<(), ClientError> {
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

    println!("p2id_serial_num_1: {:?}", p2id_serial_num_1);
    println!("p2id_serial_num_2: {:?}", p2id_serial_num_2);

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
    //  Comparing results from try_match_swapp_notes
    // ---------------------------------------------------------------------------------
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    assert_eq!(swap_data.p2id_from_1_to_2.id(), p2id_1.id());
    assert_eq!(swap_data.p2id_from_2_to_1.id(), p2id_2.id());
    assert_eq!(
        swap_data.leftover_swapp_note.clone().unwrap().id(),
        swap_note_3.id()
    );
    assert_eq!(swap_data.note1_args, swap_note_1_note_args);
    assert_eq!(swap_data.note2_args, swap_note_2_note_args);

    // ---------------------------------------------------------------------------------
    // STEP 3: Consume both SWAP notes in a single TX by the matcher & output p2id notes
    // ---------------------------------------------------------------------------------
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
            (swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_note_2.id(), Some(swap_data.note2_args)),
        ])
        .with_expected_output_notes(expected_outputs)
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(matcher_account.id(), consume_req)
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
async fn fill_counter_party_swap_notes_algo() -> Result<(), ClientError> {
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

    println!("waiting");
    wait_for_note(&mut client, &matcher_account, &swap_note_1)
        .await
        .unwrap();
    wait_for_note(&mut client, &matcher_account, &swap_note_2)
        .await
        .unwrap();

    // ---------------------------------------------------------------------------------
    //  results from try_match_swapp_notes
    // ---------------------------------------------------------------------------------
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    // ---------------------------------------------------------------------------------
    // STEP 3: Consume both SWAP notes in a single TX by the matcher & output p2id notes
    // ---------------------------------------------------------------------------------
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
            (swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_note_2.id(), Some(swap_data.note2_args)),
        ])
        .with_expected_output_notes(expected_outputs)
        .build()
        .unwrap();

    let tx_result = client
        .new_transaction(matcher_account.id(), consume_req)
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
async fn usdc_eth_orderbook_match() -> Result<(), ClientError> {
    delete_keystore_and_store().await;
    let endpoint = Endpoint::localhost();
    let mut client = instantiate_client(endpoint).await?;
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    // ──────────────────────────────────────────────────────────────────────
    // 1.  Accounts & Faucets
    // ──────────────────────────────────────────────────────────────────────
    // balances: [USDC, ETH]
    let balances = vec![
        vec![10_000, 10],       // Alice — USDC rich, small ETH
        vec![10_000, 10],       // Bob   — same
        vec![100_000, 100_000], // Matcher
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 3, 2, balances).await?;
    let alice = accounts[0].clone();
    let bob = accounts[1].clone();
    let matcher = accounts[2].clone();
    let faucet_usdc = faucets[0].clone(); // USDC
    let faucet_eth = faucets[1].clone(); // ETH

    // ──────────────────────────────────────────────────────────────────────
    // 2.  Build an orderbook (3 price levels per side)
    // ──────────────────────────────────────────────────────────────────────
    let price_levels = [2_400u64, 2_450, 2_500]; // USDC per 1 ETH
    let mut swap_notes = Vec::<Note>::new();

    for price in price_levels {
        // Alice sells 1 ETH at each level
        let n = price_to_swap_note(
            alice.id(),
            alice.id(),
            /*is_bid=*/ false,             // false = ask (selling ETH for USDC)
            price,             // price in USDC per ETH
            1,                 // quantity of ETH to sell
            &faucet_eth.id(),  // base asset (ETH)
            &faucet_usdc.id(), // quote asset (USDC)
            client.rng().draw_word(),
        );
        swap_notes.push(n);

        // Bob sells 2 500 USDC (≈1 ETH) at each level
        let n = price_to_swap_note(
            bob.id(),
            bob.id(),
            /*is_bid=*/ true,              // true = bid (buying ETH with USDC)
            price,             // price in USDC per ETH
            1,                 // quantity: roughly 1 ETH worth
            &faucet_eth.id(),  // base asset (ETH)
            &faucet_usdc.id(), // quote asset (USDC)
            client.rng().draw_word(),
        );
        swap_notes.push(n);
    }

    // Commit all swap notes on-chain
    for note in &swap_notes {
        let req = TransactionRequestBuilder::new()
            .with_own_output_notes(vec![OutputNote::Full(note.clone())])
            .build()?;
        let tx = client.new_transaction(creator_of(note), req).await?;
        client.submit_transaction(tx).await?;
    }

    // Wait for matcher to see them
    for note in &swap_notes {
        wait_for_note(&mut client, &matcher, note).await?;
    }

    // ──────────────────────────────────────────────────────────────────────
    // 3.  Repeatedly cross the book until no more matches
    // ──────────────────────────────────────────────────────────────────────
    let mut open = swap_notes.clone();
    while let Some((i, j, swap_data)) = {
        // find first crossing pair
        let mut found = None;
        'outer: for (i, n1) in open.iter().enumerate() {
            for (j, n2) in open.iter().enumerate().skip(i + 1) {
                if let Ok(Some(data)) = try_match_swapp_notes(n1, n2, matcher.id()) {
                    found = Some((i, j, data));
                    break 'outer;
                }
            }
        }
        found
    } {
        // consume the two notes (plus any leftover) in a matcher TX
        let mut expected = vec![
            swap_data.p2id_from_1_to_2.clone(),
            swap_data.p2id_from_2_to_1.clone(),
        ];
        if let Some(ref note) = swap_data.leftover_swapp_note {
            expected.push(note.clone());
        }
        expected.sort_by_key(|n| n.commitment());

        let consume_req = TransactionRequestBuilder::new()
            .with_authenticated_input_notes([
                (open[i].id(), Some(swap_data.note1_args)),
                (open[j].id(), Some(swap_data.note2_args)),
            ])
            .with_expected_output_notes(expected)
            .build()?;
        let tx = client.new_transaction(matcher.id(), consume_req).await?;
        client.submit_transaction(tx).await?;

        // remove matched notes; if leftovers exist they’re already in `expected`
        if j > i {
            open.swap_remove(j);
            open.swap_remove(i);
        } else {
            open.swap_remove(i);
            open.swap_remove(j);
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // 4.  Final balances (matcher collected all spread)
    // ──────────────────────────────────────────────────────────────────────
    client.sync_state().await?;
    let matcher_acct = client.get_account(matcher.id()).await?.unwrap();

    let binding = matcher_acct.account();
    println!(
        "Matcher profit  —  USDC: {:?}   ETH: {:?}",
        binding.vault().get_balance(faucet_usdc.id()),
        binding.vault().get_balance(faucet_eth.id())
    );

    // Ensure the book is empty
    assert!(open.is_empty(), "Orderbook not fully crossed");

    Ok(())
}

#[tokio::test]
async fn realistic_usdc_eth_orderbook_match() -> Result<(), ClientError> {
    delete_keystore_and_store().await;
    let endpoint = Endpoint::localhost();
    let mut client = instantiate_client(endpoint).await?;
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    // ──────────────────────────────────────────────────────────────────────
    // 1. Setup accounts and faucets
    // ──────────────────────────────────────────────────────────────────────
    // balances: [USDC, ETH]
    let balances = vec![
        vec![25_000, 100],    // Alice — USDC rich, some ETH
        vec![10_000, 100],    // Bob   — USDC and ETH
        vec![5_000, 100],     // Carol — USDC and ETH
        vec![50_000, 50_000], // Matcher needs assets, in the future this will be lifted as a requirement
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 4, 2, balances).await?;
    let alice = accounts[0].clone();
    let bob = accounts[1].clone();
    let carol = accounts[2].clone();
    let matcher = accounts[3].clone();
    let faucet_usdc = faucets[0].clone(); // USDC
    let faucet_eth = faucets[1].clone(); // ETH

    // ──────────────────────────────────────────────────────────────────────
    // 2. Build a realistic orderbook with multiple price levels
    // ──────────────────────────────────────────────────────────────────────
    println!("Creating a realistic USDC/ETH orderbook...");

    // Current market price is around 2500 USDC per ETH
    // Create buy orders (bids) below market price
    let bid_prices = [2450u64, 2400, 2350, 2300, 2250];
    // Create sell orders (asks) above market price
    let ask_prices = [2550u64, 2600, 2650, 2700, 2750];

    let mut swap_notes = Vec::<Note>::new();

    // Create buy orders (bids) - users want to buy ETH with USDC
    for (i, price) in bid_prices.iter().enumerate() {
        // Alice places larger orders at better prices
        let qty_usdc = 2500 - (i as u64);
        let n = price_to_swap_note(
            alice.id(),
            alice.id(),
            /*is_bid=*/ true,              // true = bid (buying ETH with USDC)
            *price,            // price in USDC per ETH
            qty_usdc / price,  // quantity of ETH to buy
            &faucet_eth.id(),  // base asset (ETH)
            &faucet_usdc.id(), // quote asset (USDC)
            client.rng().draw_word(),
        );
        swap_notes.push(n);

        // Bob places medium-sized orders
        if i < 3 {
            let qty_usdc = 1500 - (i as u64);
            let n = price_to_swap_note(
                bob.id(),
                bob.id(),
                /*is_bid=*/ true,              // true = bid (buying ETH with USDC)
                *price,            // price in USDC per ETH
                qty_usdc / price,  // quantity of ETH to buy
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }
    }

    // Create sell orders (asks) - users want to sell ETH for USDC
    for (i, price) in ask_prices.iter().enumerate() {
        // Carol places ETH sell orders
        let qty_eth = 1 - (i as u64 * 15) / 100; // 1, 0.85, 0.7, 0.55, 0.4 ETH
        if qty_eth > 0 {
            let n = price_to_swap_note(
                carol.id(),
                carol.id(),
                /*is_bid=*/ false,             // false = ask (selling ETH for USDC)
                *price,            // price in USDC per ETH
                qty_eth,           // quantity of ETH to sell
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }

        // Bob also sells some ETH at higher prices
        if i > 1 {
            let qty_eth = 2 - (i as u64 * 25) / 100; // 1.5, 1.25, 1 ETH
            let n = price_to_swap_note(
                bob.id(),
                bob.id(),
                /*is_bid=*/ false,             // false = ask (selling ETH for USDC)
                *price,            // price in USDC per ETH
                qty_eth,           // quantity of ETH to sell
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }
    }

    println!("Created {} orders in the orderbook", swap_notes.len());

    // Commit all swap notes on-chain
    for note in &swap_notes {
        let req = TransactionRequestBuilder::new()
            .with_own_output_notes(vec![OutputNote::Full(note.clone())])
            .build()?;
        let tx = client.new_transaction(creator_of(note), req).await?;
        client.submit_transaction(tx).await?;
    }

    // Wait for matcher to see all notes
    for note in &swap_notes {
        wait_for_note(&mut client, &matcher, note).await?;
    }

    // ──────────────────────────────────────────────────────────────────────
    // 3. Match orders until no more matches are possible
    // ──────────────────────────────────────────────────────────────────────
    println!("Matching orders...");

    let mut open = swap_notes.clone();
    let mut match_count = 0;

    while let Some((i, j, swap_data)) = {
        // Find first crossing pair
        let mut found = None;
        'outer: for (i, n1) in open.iter().enumerate() {
            for (j, n2) in open.iter().enumerate().skip(i + 1) {
                if let Ok(Some(data)) = try_match_swapp_notes(n1, n2, matcher.id()) {
                    found = Some((i, j, data));
                    break 'outer;
                }
            }
        }
        found
    } {
        match_count += 1;
        println!(
            "Match #{}: Crossing orders at indices {} and {}",
            match_count, i, j
        );

        // Consume the two notes (plus any leftover) in a matcher TX
        let mut expected = vec![
            swap_data.p2id_from_1_to_2.clone(),
            swap_data.p2id_from_2_to_1.clone(),
        ];
        if let Some(ref note) = swap_data.leftover_swapp_note {
            expected.push(note.clone());
        }
        expected.sort_by_key(|n| n.commitment());

        let consume_req = TransactionRequestBuilder::new()
            .with_authenticated_input_notes([
                (open[i].id(), Some(swap_data.note1_args)),
                (open[j].id(), Some(swap_data.note2_args)),
            ])
            .with_expected_output_notes(expected)
            .build()?;
        let tx = client.new_transaction(matcher.id(), consume_req).await?;
        client.submit_transaction(tx).await?;

        // Remove matched notes; if leftovers exist they're already in `expected`
        if j > i {
            open.swap_remove(j);
            open.swap_remove(i);
        } else {
            open.swap_remove(i);
            open.swap_remove(j);
        }

        // If there's a leftover note, add it back to the open orders
        if let Some(leftover) = swap_data.leftover_swapp_note {
            // Wait for the matcher to see the leftover note
            wait_for_note(&mut client, &matcher, &leftover).await?;
            open.push(leftover);
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // 4. Final balances and statistics
    // ──────────────────────────────────────────────────────────────────────
    client.sync_state().await?;

    // Get final balances
    let matcher_acct = client.get_account(matcher.id()).await?.unwrap();
    let alice_acct = client.get_account(alice.id()).await?.unwrap();
    let bob_acct = client.get_account(bob.id()).await?.unwrap();
    let carol_acct = client.get_account(carol.id()).await?.unwrap();

    let matcher_binding = matcher_acct.account();
    let alice_binding = alice_acct.account();
    let bob_binding = bob_acct.account();
    let carol_binding = carol_acct.account();

    println!("\nFinal balances:");
    println!(
        "Matcher profit  —  USDC: {:?}   ETH: {:?}",
        matcher_binding.vault().get_balance(faucet_usdc.id()),
        matcher_binding.vault().get_balance(faucet_eth.id())
    );
    println!(
        "Alice balance   —  USDC: {:?}   ETH: {:?}",
        alice_binding.vault().get_balance(faucet_usdc.id()),
        alice_binding.vault().get_balance(faucet_eth.id())
    );
    println!(
        "Bob balance     —  USDC: {:?}   ETH: {:?}",
        bob_binding.vault().get_balance(faucet_usdc.id()),
        bob_binding.vault().get_balance(faucet_eth.id())
    );
    println!(
        "Carol balance   —  USDC: {:?}   ETH: {:?}",
        carol_binding.vault().get_balance(faucet_usdc.id()),
        carol_binding.vault().get_balance(faucet_eth.id())
    );

    println!("\nMatched {} order pairs", match_count);
    println!("Remaining open orders: {}", open.len());

    // Print details of remaining orders
    if !open.is_empty() {
        println!("\nRemaining orders:");
        for (i, note) in open.iter().enumerate() {
            if let Ok((offered, requested)) = decompose_swapp_note(note) {
                let creator_id = creator_of(note);
                let creator_name = if creator_id == alice.id() {
                    "Alice"
                } else if creator_id == bob.id() {
                    "Bob"
                } else if creator_id == carol.id() {
                    "Carol"
                } else {
                    "Unknown"
                };

                println!(
                    "  Order #{}: {} offers {} × {} and requests {} × {}",
                    i + 1,
                    creator_name,
                    offered.amount(),
                    offered.faucet_id(),
                    requested.amount(),
                    requested.faucet_id()
                );
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn usdc_eth_orderbook_depth_chart() -> Result<(), ClientError> {
    delete_keystore_and_store().await;
    let endpoint = Endpoint::localhost();
    let mut client = instantiate_client(endpoint).await?;
    let keystore = FilesystemKeyStore::new("./keystore".into()).unwrap();

    // ──────────────────────────────────────────────────────────────────────
    // 1. Setup accounts and faucets
    // ──────────────────────────────────────────────────────────────────────
    // balances: [USDC, ETH]
    let balances = vec![
        vec![500_000, 1000],    // Alice — USDC and ETH
        vec![500_000, 1000],    // Bob   — USDC and ETH
        vec![500_000, 1000],    // Carol — USDC and ETH
        vec![500_000, 500_000], // Matcher needs assets, in the future this will be lifted as a requirement
    ];
    let (accounts, faucets) =
        setup_accounts_and_faucets(&mut client, keystore, 4, 2, balances).await?;
    let alice = accounts[0].clone();
    let bob = accounts[1].clone();
    let carol = accounts[2].clone();
    let matcher = accounts[3].clone();
    let faucet_usdc = faucets[0].clone(); // USDC
    let faucet_eth = faucets[1].clone(); // ETH

    // ──────────────────────────────────────────────────────────────────────
    // 2. Build a realistic orderbook with multiple price levels
    // ──────────────────────────────────────────────────────────────────────
    println!("Creating a realistic USDC/ETH orderbook...");

    // Current market price is around 2500 USDC per ETH
    // Create a tighter spread with smaller increments near the mid-price
    // and larger increments further away to create a hockey stick shape

    // Mid price is 2500
    // Create buy orders (bids) below market price with tighter spread
    let bid_prices = [
        2500u64, 2499, 2498, 2495, 2490, 2480, 2460, 2430, 2390, 2340, 2280,
    ];

    // Create sell orders (asks) above market price with tighter spread
    let ask_prices = [
        2500u64, 2505, 2507, 2510, 2520, 2540, 2570, 2610, 2660, 2720, 2800,
    ];

    let mut swap_notes = Vec::<Note>::new();

    // Create buy orders (bids) - users want to buy ETH with USDC
    // Order sizes follow hockey stick pattern - smaller near mid-price, larger away from it
    for (i, price) in bid_prices.iter().enumerate() {
        // Calculate order size based on distance from mid-price
        // Exponential growth as we move away from mid-price
        let base_eth = 1u64; // Base size in ETH
        let multiplier = 1.0 + (i as f64 * 0.3); // Grows with distance from mid-price
        let eth_amount = (base_eth as f64 * multiplier).ceil() as u64;

        // Alice places orders at all price levels
        let n = price_to_swap_note(
            alice.id(),
            alice.id(),
            /*is_bid=*/ true,              // true = bid (buying ETH with USDC)
            *price,            // price in USDC per ETH
            eth_amount,        // quantity of ETH to buy
            &faucet_eth.id(),  // base asset (ETH)
            &faucet_usdc.id(), // quote asset (USDC)
            client.rng().draw_word(),
        );
        swap_notes.push(n);

        // Bob places orders at some price levels
        if i % 2 == 0 && i < 8 {
            // Bob places larger orders at certain price points
            let bob_multiplier = 0.8 + (i as f64 * 0.3);
            let bob_eth = (base_eth as f64 * bob_multiplier * 1.5).ceil() as u64;

            let n = price_to_swap_note(
                bob.id(),
                bob.id(),
                /*is_bid=*/ true,              // true = bid (buying ETH with USDC)
                *price,            // price in USDC per ETH
                bob_eth,           // quantity of ETH to buy
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }

        // Carol occasionally places small orders near the mid-price
        if i < 3 {
            let carol_eth = base_eth;

            let n = price_to_swap_note(
                carol.id(),
                carol.id(),
                /*is_bid=*/ true,              // true = bid (buying ETH with USDC)
                *price,            // price in USDC per ETH
                carol_eth,         // quantity of ETH to buy
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }
    }

    // Create sell orders (asks) - users want to sell ETH for USDC
    // Order sizes follow hockey stick pattern - smaller near mid-price, larger away from it
    for (i, price) in ask_prices.iter().enumerate() {
        // Calculate order size based on distance from mid-price
        // Exponential growth as we move away from mid-price
        let base_size = 1u128; // Base size in ETH
        let multiplier = 1.0 + (i as f64 * 0.3); // Grows with distance from mid-price
        let qty_eth = (base_size as f64 * multiplier).ceil() as u64;

        // Carol places orders at all price levels
        let n = price_to_swap_note(
            carol.id(),
            carol.id(),
            /*is_bid=*/ false,             // false = ask (selling ETH for USDC)
            *price,            // price in USDC per ETH
            qty_eth,           // quantity of ETH to sell
            &faucet_eth.id(),  // base asset (ETH)
            &faucet_usdc.id(), // quote asset (USDC)
            client.rng().draw_word(),
        );
        swap_notes.push(n);

        // Bob places orders at some price levels
        if i % 2 == 0 && i < 8 {
            // Bob places larger orders at certain price points
            let bob_multiplier = 0.8 + (i as f64 * 0.25);
            let bob_qty = (base_size as f64 * bob_multiplier * 1.5).ceil() as u64;

            let n = price_to_swap_note(
                bob.id(),
                bob.id(),
                /*is_bid=*/ false,             // false = ask (selling ETH for USDC)
                *price,            // price in USDC per ETH
                bob_qty,           // quantity of ETH to sell
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }

        // Alice occasionally places small orders near the mid-price
        if i < 3 {
            let alice_qty = 1u64;
            let n = price_to_swap_note(
                alice.id(),
                alice.id(),
                /*is_bid=*/ false,             // false = ask (selling ETH for USDC)
                *price,            // price in USDC per ETH
                alice_qty,         // quantity of ETH to sell
                &faucet_eth.id(),  // base asset (ETH)
                &faucet_usdc.id(), // quote asset (USDC)
                client.rng().draw_word(),
            );
            swap_notes.push(n);
        }
    }

    println!("Created {} orders in the orderbook", swap_notes.len());

    // Commit all swap notes on-chain
    for note in &swap_notes {
        let req = TransactionRequestBuilder::new()
            .with_own_output_notes(vec![OutputNote::Full(note.clone())])
            .build()?;
        let tx = client.new_transaction(creator_of(note), req).await?;
        client.submit_transaction(tx).await?;
    }

    // Wait for matcher to see all notes
    for note in &swap_notes {
        wait_for_note(&mut client, &matcher, note).await?;
    }

    // ──────────────────────────────────────────────────────────────────────
    // 3. Match some orders to create a more realistic orderbook state
    // ──────────────────────────────────────────────────────────────────────
    println!("Matching some orders to create a realistic orderbook state...");

    // Create a copy of the swap notes for matching
    let mut open_orders = swap_notes.clone();
    let mut matched_count = 0;
    let max_matches = 5; // Limit the number of matches to keep some orders in the book

    // Match orders until we reach the limit or no more matches are possible
    while matched_count < max_matches {
        // Find first crossing pair
        let mut found = None;
        'outer: for (i, n1) in open_orders.iter().enumerate() {
            for (j, n2) in open_orders.iter().enumerate().skip(i + 1) {
                if let Ok(Some(data)) = try_match_swapp_notes(n1, n2, matcher.id()) {
                    found = Some((i, j, data));
                    break 'outer;
                }
            }
        }

        // If no crossing pair is found, break the loop
        let (i, j, swap_data) = match found {
            Some(data) => data,
            None => break,
        };

        matched_count += 1;
        println!("Match #{}: Crossing orders", matched_count);

        // Consume the two notes (plus any leftover) in a matcher TX
        let mut expected = vec![
            swap_data.p2id_from_1_to_2.clone(),
            swap_data.p2id_from_2_to_1.clone(),
        ];
        if let Some(ref note) = swap_data.leftover_swapp_note {
            expected.push(note.clone());
        }
        expected.sort_by_key(|n| n.commitment());

        let consume_req = TransactionRequestBuilder::new()
            .with_authenticated_input_notes([
                (open_orders[i].id(), Some(swap_data.note1_args)),
                (open_orders[j].id(), Some(swap_data.note2_args)),
            ])
            .with_expected_output_notes(expected)
            .build()?;
        let tx = client.new_transaction(matcher.id(), consume_req).await?;
        client.submit_transaction(tx).await?;

        // Remove matched notes; if leftovers exist they're already in `expected`
        if j > i {
            open_orders.swap_remove(j);
            open_orders.swap_remove(i);
        } else {
            open_orders.swap_remove(i);
            open_orders.swap_remove(j);
        }

        // If there's a leftover note, add it back to the open orders
        if let Some(ref leftover) = swap_data.leftover_swapp_note {
            // Wait for the matcher to see the leftover note
            wait_for_note(&mut client, &matcher, leftover).await?;
            open_orders.push(leftover.clone());

            // Also add it to the original swap_notes list to ensure it's included in the analysis
            swap_notes.push(leftover.clone());
        }
    }

    println!("Matched {} order pairs", matched_count);

    // ──────────────────────────────────────────────────────────────────────
    // 4. Analyze the orderbook and create a depth chart using the refactored function
    // ──────────────────────────────────────────────────────────────────────
    let account_names = [
        (alice.id(), "Alice"),
        (bob.id(), "Bob"),
        (carol.id(), "Carol"),
    ];

    generate_depth_chart(
        &swap_notes,
        &faucet_usdc.id(),
        &faucet_eth.id(),
        &account_names,
    );

    Ok(())
}
