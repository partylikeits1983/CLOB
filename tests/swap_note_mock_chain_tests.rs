use miden_client::{
    asset::{Asset, FungibleAsset},
    note::NoteType,
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1,
    Word,
};
use miden_clob::{create_partial_swap_note, try_match_swapp_notes};
use miden_testing::{Auth, MockChain, TransactionContextBuilder};

use miden_objects::{
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2, transaction::OutputNote,
};

#[test]
fn p2id_script_multiple_assets() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    // Create assets
    let fungible_asset_1: Asset = FungibleAsset::mock(123);
    let fungible_asset_2: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap(), 456)
            .unwrap()
            .into();

    // Create sender and target account
    let sender_account = builder.add_existing_wallet(Auth::BasicAuth)?;
    let target_account = builder.add_existing_wallet(Auth::BasicAuth)?;

    // Create the note
    let note = builder.add_p2id_note(
        sender_account.id(),
        target_account.id(),
        &[fungible_asset_1, fungible_asset_2],
        NoteType::Public,
    )?;

    let mut mock_chain = builder.build()?;
    mock_chain.prove_next_block()?;

    println!("p2id script hash: {:?}", note.script().root());
    Ok(())
}

#[tokio::test]
async fn swapp_match_mock_chain() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    // Initialize assets & accounts
    let asset_a: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap(), 100)
            .unwrap()
            .into();
    let asset_b: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap(), 100)
            .unwrap()
            .into();

    // Create sender and target and matcher account
    let alice_account = builder.add_existing_wallet(Auth::BasicAuth)?;
    let bob_account = builder.add_existing_wallet(Auth::BasicAuth)?;
    let matcher_account = builder.add_existing_wallet(Auth::BasicAuth)?;

    // SWAPP NOTE 1
    let swap_note_1_asset_a: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap(), 100)
            .unwrap()
            .into();
    let swap_note_1_asset_b: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap(), 100)
            .unwrap()
            .into();

    let swap_note_1 = create_partial_swap_note(
        alice_account.id(),         // creator of the order
        alice_account.id(),         // last account to "fill the order"
        swap_note_1_asset_a.into(), // offered asset (selling)
        swap_note_1_asset_b.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    // SWAPP NOTE 2
    let swap_note_2_asset_a: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap(), 100)
            .unwrap()
            .into();
    let swap_note_2_asset_b: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap(), 100)
            .unwrap()
            .into();

    let swap_note_2 = create_partial_swap_note(
        bob_account.id(),           // creator of the order
        bob_account.id(),           // last account to "fill the order"
        swap_note_2_asset_b.into(), // offered asset (selling)
        swap_note_2_asset_a.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    let swapp_note1_output = OutputNote::Full(swap_note_1.clone());
    let swapp_note2_output = OutputNote::Full(swap_note_2.clone());

    builder.add_note(swapp_note1_output);
    builder.add_note(swapp_note2_output);

    let mut mock_chain = builder.build()?;
    mock_chain.prove_next_block()?;

    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    println!("built notes, executing tx");

    let mut outputs = vec![
        OutputNote::Full(swap_data.p2id_from_2_to_1),
        OutputNote::Full(swap_data.p2id_from_1_to_2),
    ];

    if let Some(ref note) = swap_data.leftover_swapp_note {
        outputs.push(OutputNote::Full(note.clone()));
    }

    // CONSTRUCT AND EXECUTE TX (Success - Target Account)
    let tx_inputs = mock_chain.get_transaction_inputs(
        matcher_account.clone(),
        None,
        &[swap_note_1.id(), swap_note_2.id()],
        &[],
    )?;

    let tx_context = TransactionContextBuilder::new(matcher_account.clone())
        .tx_inputs(tx_inputs)
        .extend_expected_output_notes(outputs)
        .build()?;

    let executed_transaction_1 = tx_context.execute().await?;

    let target_account = mock_chain.add_pending_executed_transaction(&executed_transaction_1)?;

    println!(
        "asset a: {:?} asset b: {:?}",
        target_account
            .vault()
            .get_balance(asset_a.unwrap_fungible().faucet_id()),
        target_account
            .vault()
            .get_balance(asset_b.unwrap_fungible().faucet_id())
    );

    Ok(())
}

#[tokio::test]
async fn swapp_match_mock_chain_exact_error_values() -> anyhow::Result<()> {
    // Test with the exact values from the error message:
    // Note 1: offers 10, wants 45290
    // Note 2: offers 54360, wants 12

    let mut builder = MockChain::builder();

    // Create faucets for the two assets
    let faucet_a = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let faucet_b = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();

    // Initialize assets for the matcher account (needs enough to cover both sides)
    let asset_a_matcher: Asset = FungibleAsset::new(faucet_a, 100000000000).unwrap().into();
    let asset_b_matcher: Asset = FungibleAsset::new(faucet_b, 100000000000).unwrap().into();

    // Create accounts
    let alice_account = builder.add_existing_wallet(Auth::BasicAuth)?;
    let bob_account = builder.add_existing_wallet(Auth::BasicAuth)?;
    let matcher_account = builder.add_existing_wallet(Auth::BasicAuth)?;

    // SWAPP NOTE 1: Alice offers 10 B, wants 45290 A (high price per A)
    let swap_note_1 = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        FungibleAsset::new(faucet_b, 10).unwrap().into(), // offered: 10 B
        FungibleAsset::new(faucet_a, 45290).unwrap().into(), // wanted: 45290 A
        *Word::default(),
        0,
    )
    .unwrap();

    // SWAPP NOTE 2: Bob offers 54360 A, wants 12 B (low price per A - better deal)
    let swap_note_2 = create_partial_swap_note(
        bob_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a, 54360).unwrap().into(), // offered: 54360 A
        FungibleAsset::new(faucet_b, 12).unwrap().into(),    // wanted: 12 B
        *Word::default(),
        0,
    )
    .unwrap();

    // Add notes to the chain
    let swapp_note1_output = OutputNote::Full(swap_note_1.clone());
    let swapp_note2_output = OutputNote::Full(swap_note_2.clone());

    builder.add_note(swapp_note1_output);
    builder.add_note(swapp_note2_output);

    let mut mock_chain = builder.build()?;
    mock_chain.prove_next_block()?;

    println!("\n=== Testing exact error values ===");
    println!(
        "Note 1: offers 10 B, wants 45290 A (ratio: {})",
        45290.0 / 10.0
    );
    println!(
        "Note 2: offers 54360 A, wants 12 B (ratio: {})",
        12.0 / 54360.0
    );

    // Try to match the notes
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account.id())
        .unwrap()
        .expect("orders should cross");

    println!("\n=== Match results ===");

    // Check P2ID notes
    let p2id_to_alice = swap_data
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    let p2id_to_bob = swap_data
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    println!(
        "P2ID to Alice: {} of faucet {:?}",
        p2id_to_alice.amount(),
        p2id_to_alice.faucet_id()
    );
    println!(
        "P2ID to Bob: {} of faucet {:?}",
        p2id_to_bob.amount(),
        p2id_to_bob.faucet_id()
    );

    // Check leftover
    if let Some(ref leftover) = swap_data.leftover_swapp_note {
        let (offered, requested) = miden_clob::decompose_swapp_note(leftover).unwrap();
        println!("Leftover SWAPP note:");
        println!(
            "  - Offers: {} of faucet {:?}",
            offered.amount(),
            offered.faucet_id()
        );
        println!(
            "  - Wants: {} of faucet {:?}",
            requested.amount(),
            requested.faucet_id()
        );
    } else {
        println!("No leftover SWAPP note (complete fill)");
    }

    // Build outputs for transaction
    let mut outputs = vec![
        OutputNote::Full(swap_data.p2id_from_2_to_1),
        OutputNote::Full(swap_data.p2id_from_1_to_2),
    ];

    if let Some(ref note) = swap_data.leftover_swapp_note {
        outputs.push(OutputNote::Full(note.clone()));
    }

    println!("\n=== Executing transaction ===");

    // Execute the matching transaction
    let tx_inputs = mock_chain.get_transaction_inputs(
        matcher_account.clone(),
        None,
        &[swap_note_1.id(), swap_note_2.id()],
        &[],
    )?;

    let tx_context = TransactionContextBuilder::new(matcher_account.clone())
        .tx_inputs(tx_inputs)
        .extend_note_args(
            [
                (swap_note_1.id(), Word::from(swap_data.note1_args)),
                (swap_note_2.id(), Word::from(swap_data.note2_args)),
            ]
            .into(),
        )
        .extend_expected_output_notes(outputs)
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    println!(
        "cycles: {:?}",
        executed_transaction.measurements().total_cycles()
    );

    let final_matcher_account =
        mock_chain.add_pending_executed_transaction(&executed_transaction)?;

    // There should be a leftover from Bob's order
    assert!(
        swap_data.leftover_swapp_note.is_some(),
        "Bob's order should be partially filled"
    );

    println!(
        "balance a: {:?}",
        final_matcher_account.vault().get_balance(faucet_a)
    );
    println!(
        "balance b: {:?}",
        final_matcher_account.vault().get_balance(faucet_b)
    );

    println!("\n=== Test passed! ===");

    Ok(())
}
