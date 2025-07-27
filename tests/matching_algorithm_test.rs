use std::time::Duration;

use miden_client::{
    asset::FungibleAsset,
    crypto::FeltRng,
    keystore::FilesystemKeyStore,
    note::NoteType,
    rpc::Endpoint,
    transaction::{OutputNote, TransactionRequestBuilder},
    ClientError, Felt,
};
use miden_objects::note::NoteDetails;
use tokio::time::sleep;

use miden_clob::common::{
    create_p2id_note, create_partial_swap_note, delete_keystore_and_store, get_p2id_serial_num,
    instantiate_client, setup_accounts_and_faucets, try_match_swapp_notes, wait_for_note,
};

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
        .own_output_notes(vec![OutputNote::Full(swap_note_1.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swap_note_2.clone())])
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
        (
            NoteDetails::from(swap_data.p2id_from_1_to_2.clone()),
            swap_data.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data.p2id_from_2_to_1.clone()),
            swap_data.p2id_from_2_to_1.metadata().tag(),
        ),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }

    let mut expected_output_recipients = vec![
        swap_data.p2id_from_1_to_2.recipient().clone(),
        swap_data.p2id_from_2_to_1.recipient().clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }

    let consume_req = TransactionRequestBuilder::new()
        .authenticated_input_notes([
            (swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_note_2.id(), Some(swap_data.note2_args)),
        ])
        .expected_future_notes(expected_outputs)
        .expected_output_recipients(expected_output_recipients)
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
        vec![100_000_000, 100_000_000], // For account[0] => Alice
        vec![100_000_000, 100_000_000], // For account[0] => Bob
        vec![100_000_000, 100_000_000], // For account[0] => matcher
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
    let swap_note_1_asset_a = FungibleAsset::new(faucet_a.id(), 600).unwrap();
    let swap_note_1_asset_b = FungibleAsset::new(faucet_b.id(), 1455600).unwrap();
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

    let swap_note_2_asset_a = FungibleAsset::new(faucet_a.id(), 71).unwrap();
    let swap_note_2_asset_b = FungibleAsset::new(faucet_b.id(), 173737).unwrap();
    let swap_note_2_serial_num = client.rng().draw_word();
    let swap_note_2 = create_partial_swap_note(
        bob_account.id(),
        bob_account.id(),
        swap_note_2_asset_b.into(),
        swap_note_2_asset_a.into(),
        swap_note_2_serial_num,
        1,
    )
    .unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swap_note_1.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swap_note_2.clone())])
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
        (
            NoteDetails::from(swap_data.p2id_from_1_to_2.clone()),
            swap_data.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data.p2id_from_2_to_1.clone()),
            swap_data.p2id_from_2_to_1.metadata().tag(),
        ),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }

    let mut expected_output_recipients = vec![
        swap_data.p2id_from_1_to_2.recipient().clone(),
        swap_data.p2id_from_2_to_1.recipient().clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }

    let consume_req = TransactionRequestBuilder::new()
        .authenticated_input_notes([
            (swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_note_2.id(), Some(swap_data.note2_args)),
        ])
        .expected_future_notes(expected_outputs)
        .expected_output_recipients(expected_output_recipients)
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
        .own_output_notes(vec![OutputNote::Full(swap_note_1.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swap_note_2.clone())])
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
        (
            NoteDetails::from(swap_data.p2id_from_1_to_2.clone()),
            swap_data.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data.p2id_from_2_to_1.clone()),
            swap_data.p2id_from_2_to_1.metadata().tag(),
        ),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }

    let mut expected_output_recipients = vec![
        swap_data.p2id_from_1_to_2.recipient().clone(),
        swap_data.p2id_from_2_to_1.recipient().clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }

    let consume_req = TransactionRequestBuilder::new()
        .authenticated_input_notes([
            (swap_note_1.id(), Some(swap_data.note1_args)),
            (swap_note_2.id(), Some(swap_data.note2_args)),
        ])
        .expected_future_notes(expected_outputs)
        .expected_output_recipients(expected_output_recipients)
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
        .own_output_notes(vec![OutputNote::Full(swap_note_1.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![
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
        (
            NoteDetails::from(swap_data.p2id_from_1_to_2.clone()),
            swap_data.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data.p2id_from_2_to_1.clone()),
            swap_data.p2id_from_2_to_1.metadata().tag(),
        ),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }

    let mut expected_output_recipients = vec![
        swap_data.p2id_from_1_to_2.recipient().clone(),
        swap_data.p2id_from_2_to_1.recipient().clone(),
    ];
    if let Some(ref note) = swap_data.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }

    let consume_req = TransactionRequestBuilder::new()
        .unauthenticated_input_notes([
            (swap_note_1, Some(swap_data.note1_args)),
            (swap_note_2, Some(swap_data.note2_args)),
        ])
        .expected_future_notes(expected_outputs)
        .expected_output_recipients(expected_output_recipients)
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
        (
            NoteDetails::from(swap_data.p2id_from_1_to_2.clone()),
            swap_data.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data.p2id_from_2_to_1.clone()),
            swap_data.p2id_from_2_to_1.metadata().tag(),
        ),
    ];
    if let Some(ref note) = swap_data_1.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }

    let mut expected_output_recipients = vec![
        swap_data_1.p2id_from_1_to_2.recipient().clone(),
        swap_data_1.p2id_from_2_to_1.recipient().clone(),
    ];
    if let Some(ref note) = swap_data_1.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }

    let consume_req = TransactionRequestBuilder::new()
        .unauthenticated_input_notes([
            (swap_data_1.swap_note_1, Some(swap_data_1.note1_args)),
            (swap_data_1.swap_note_2, Some(swap_data_1.note2_args)),
        ])
        .expected_future_notes(expected_outputs)
        .expected_output_recipients(expected_output_recipients)
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
async fn multi_order_fill_test() -> Result<(), ClientError> {
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
        vec![10000, 10000], // For account[0] => Alice
        vec![10000, 10000], // For account[0] => Bob
        vec![10000, 10000], // For account[0] => matcher
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

    let swap_note_3_asset_a = FungibleAsset::new(faucet_a.id(), 80).unwrap();
    let swap_note_3_asset_b = FungibleAsset::new(faucet_b.id(), 80).unwrap();
    let swap_note_3_serial_num = client.rng().draw_word();
    let swap_note_3 = create_partial_swap_note(
        alice_account.id(),         // creator of the order
        alice_account.id(),         // last account to "fill the order"
        swap_note_3_asset_a.into(), // offered asset (selling)
        swap_note_3_asset_b.into(), // requested asset (buying)
        swap_note_3_serial_num,     // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    let swap_note_4_asset_a = FungibleAsset::new(faucet_a.id(), 30).unwrap();
    let swap_note_4_asset_b = FungibleAsset::new(faucet_b.id(), 30).unwrap();
    let swap_note_4_serial_num = client.rng().draw_word();
    let swap_note_4 = create_partial_swap_note(
        bob_account.id(),
        bob_account.id(),
        swap_note_4_asset_b.into(),
        swap_note_4_asset_a.into(),
        swap_note_4_serial_num,
        0,
    )
    .unwrap();

    // put this in a for loop
    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![
            OutputNote::Full(swap_note_1.clone()),
            OutputNote::Full(swap_note_3.clone()),
        ])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(alice_account.id(), note_creation_request)
        .await
        .unwrap();
    client.submit_transaction(tx_result).await.unwrap();

    let note_creation_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![
            OutputNote::Full(swap_note_2.clone()),
            OutputNote::Full(swap_note_4.clone()),
        ])
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
    let swap_data_1 = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    let swap_data_2 = try_match_swapp_notes(&swap_note_3, &swap_note_4, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    // ---------------------------------------------------------------------------------
    // STEP 3: Consume both SWAP notes in a single TX by the matcher & output p2id notes
    // ---------------------------------------------------------------------------------
    // Construct expected output notes exactly like the test
    let mut expected_outputs = vec![
        (
            NoteDetails::from(swap_data_1.p2id_from_1_to_2.clone()),
            swap_data_1.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data_1.p2id_from_2_to_1.clone()),
            swap_data_1.p2id_from_2_to_1.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data_2.p2id_from_1_to_2.clone()),
            swap_data_2.p2id_from_1_to_2.metadata().tag(),
        ),
        (
            NoteDetails::from(swap_data_2.p2id_from_2_to_1.clone()),
            swap_data_2.p2id_from_2_to_1.metadata().tag(),
        ),
    ];
    if let Some(ref note) = swap_data_1.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }
    if let Some(ref note) = swap_data_2.leftover_swapp_note {
        expected_outputs.push((NoteDetails::from(note.clone()), note.metadata().tag()));
    }

    let mut expected_output_recipients = vec![
        swap_data_1.p2id_from_1_to_2.recipient().clone(),
        swap_data_1.p2id_from_2_to_1.recipient().clone(),
        swap_data_2.p2id_from_1_to_2.recipient().clone(),
        swap_data_2.p2id_from_2_to_1.recipient().clone(),
    ];
    if let Some(ref note) = swap_data_1.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }
    if let Some(ref note) = swap_data_2.leftover_swapp_note {
        expected_output_recipients.push(note.recipient().clone());
    }

    let consume_req = TransactionRequestBuilder::new()
        .authenticated_input_notes([
            (swap_data_1.swap_note_1.id(), Some(swap_data_1.note1_args)),
            (swap_data_1.swap_note_2.id(), Some(swap_data_1.note2_args)),
            (swap_data_2.swap_note_1.id(), Some(swap_data_2.note1_args)),
            (swap_data_2.swap_note_2.id(), Some(swap_data_2.note2_args)),
        ])
        .expected_future_notes(expected_outputs)
        .expected_output_recipients(expected_output_recipients)
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
