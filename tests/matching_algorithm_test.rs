use std::time::{Duration, Instant};

use miden_client::{
    ClientError, Felt,
    asset::FungibleAsset,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{Note, NoteType},
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionRequestBuilder},
};
use tokio::time::sleep;

use std::sync::Arc;

use miden_clob::common::{
    compute_partial_swapp, create_p2id_note, create_partial_swap_note, creator_of,
    decompose_swapp_note, delete_keystore_and_store, generate_depth_chart, get_p2id_serial_num,
    get_swapp_note, instantiate_client, price_to_swap_note, setup_accounts_and_faucets,
    try_match_swapp_notes, wait_for_note,
};
use miden_crypto::rand::FeltRng;

#[tokio::test]
async fn swap_note_basic_test_without_matcher() -> Result<(), ClientError> {
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
async fn partial_fill_counter_party_swap_notes_with_matching_algorithm() -> Result<(), ClientError>
{
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
async fn fill_counter_party_swap_notes_complete_fill_algorithm() -> Result<(), ClientError> {
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

    let swap_note_2_asset_a = FungibleAsset::new(faucet_a.id(), 100).unwrap();
    let swap_note_2_asset_b = FungibleAsset::new(faucet_b.id(), 100).unwrap();
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
    println!("calling try_match_swapp_notes");
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
async fn fill_partial_filled_swap_note_test() -> Result<(), ClientError> {
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

    let swap_note_3_asset_a = FungibleAsset::new(faucet_a.id(), 25).unwrap();
    let swap_note_3_asset_b = FungibleAsset::new(faucet_b.id(), 25).unwrap();
    let swap_note_3_serial_num = client.rng().draw_word();
    let swap_note_3 = create_partial_swap_note(
        bob_account.id(),
        bob_account.id(),
        swap_note_3_asset_b.into(),
        swap_note_3_asset_a.into(),
        swap_note_3_serial_num,
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
        .with_own_output_notes(vec![
            OutputNote::Full(swap_note_2.clone()),
            OutputNote::Full(swap_note_3.clone()),
        ])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(bob_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    // println!("waiting 10 secs");

    sleep(Duration::from_secs(10)).await;

    wait_for_note(&mut client, &matcher_account, &swap_note_1)
        .await
        .unwrap();

    println!("continue");

    // ---------------------------------------------------------------------------------
    //  results from try_match_swapp_notes
    // ---------------------------------------------------------------------------------
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    println!("p2id: {:?}", swap_data.p2id_from_1_to_2.script().root());

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
        .with_unauthenticated_input_notes([
            (swap_note_1, Some(swap_data.note1_args)),
            (swap_note_2, Some(swap_data.note2_args)),
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

    println!("first fill success");

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

    // FILLING OUTPUT NOTE
    let swap_note_4 = swap_data.leftover_swapp_note.unwrap();

    // ---------------------------------------------------------------------------------
    //  SECOND FILL ATTEMPT
    // ---------------------------------------------------------------------------------
    let swap_data_1 = try_match_swapp_notes(&swap_note_3, &swap_note_4, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    let mut expected_outputs = vec![
        swap_data_1.p2id_from_1_to_2.clone(),
        swap_data_1.p2id_from_2_to_1.clone(),
    ];
    if let Some(ref note) = swap_data_1.leftover_swapp_note {
        expected_outputs.push(note.clone());
    }
    expected_outputs.sort_by_key(|n| n.commitment());

    let consume_req = TransactionRequestBuilder::new()
        .with_unauthenticated_input_notes([
            (swap_data_1.swap_note_1, Some(swap_data_1.note1_args)),
            (swap_data_1.swap_note_2, Some(swap_data_1.note2_args)),
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
