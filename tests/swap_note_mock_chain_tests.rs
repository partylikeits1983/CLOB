use miden_client::{
    account::AccountId,
    asset::{Asset, FungibleAsset},
    note::NoteType,
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1,
    Word,
};
use miden_clob::{create_partial_swap_note, try_match_swapp_notes};
use miden_testing::{Auth, MockChain};

use miden_objects::{
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2, transaction::OutputNote,
};

#[test]
fn p2id_script_multiple_assets() {
    let mut mock_chain = MockChain::new();

    // Create assets
    let fungible_asset_1: Asset = FungibleAsset::mock(123);
    let fungible_asset_2: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap(), 456)
            .unwrap()
            .into();

    // Create sender and target account
    let sender_account = mock_chain.add_pending_new_wallet(Auth::BasicAuth);
    let target_account = mock_chain.add_pending_existing_wallet(Auth::BasicAuth, vec![]);

    // Create the note
    let note = mock_chain
        .add_pending_p2id_note(
            sender_account.id(),
            target_account.id(),
            &[fungible_asset_1, fungible_asset_2],
            NoteType::Public,
        )
        .unwrap();

    mock_chain.prove_next_block();

    println!("p2id script hash: {:?}", note.script().root());
}

#[tokio::test]
async fn swapp_match_mock_chain() -> anyhow::Result<()> {
    let mut mock_chain = MockChain::new();
    mock_chain.prove_until_block(1u32)?;

    // Initialize assets & accounts
    let asset_a: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap(), 100)
            .unwrap()
            .into();
    let asset_b: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap(), 100)
            .unwrap()
            .into();

    // Create sender and target and malicious account
    let alice_account = mock_chain.add_pending_existing_wallet(Auth::BasicAuth, vec![]);
    let bob_account = mock_chain.add_pending_existing_wallet(Auth::BasicAuth, vec![]);
    let matcher_account =
        mock_chain.add_pending_existing_wallet(Auth::BasicAuth, vec![asset_a, asset_b]);

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
        Word::default(),            // serial number of the order
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
        Word::default(),            // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    let swapp_note1_output = OutputNote::Full(swap_note_1.clone());
    let swapp_note2_output = OutputNote::Full(swap_note_2.clone());

    mock_chain.add_pending_note(swapp_note1_output);
    mock_chain.add_pending_note(swapp_note2_output);
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
    let executed_transaction_1 = mock_chain
        .build_tx_context(
            matcher_account.id(),
            &[swap_note_1.id(), swap_note_2.id()],
            &[],
        )?
        .extend_expected_output_notes(outputs)
        .build()?
        .execute()
        .await?;

    let target_account = mock_chain.add_pending_executed_transaction(&executed_transaction_1)?;

    println!(
        "asset a: {:?} asset b: {:?}",
        target_account
            .vault()
            .get_balance(AccountId::try_from(asset_a.unwrap_fungible().faucet_id())?),
        target_account
            .vault()
            .get_balance(AccountId::try_from(asset_b.unwrap_fungible().faucet_id())?)
    );

    Ok(())
}

#[tokio::test]
async fn swapp_match_mock_chain_exact_error_values() -> anyhow::Result<()> {
    // Test with the exact values from the error message:
    // Note 1: offers 10, wants 45290
    // Note 2: offers 54360, wants 12

    let mut mock_chain = MockChain::new();
    mock_chain.prove_until_block(1u32)?;

    // Create faucets for the two assets
    let faucet_a = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let faucet_b = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();

    // Initialize assets for the matcher account (needs enough to cover both sides)
    let asset_a_matcher: Asset = FungibleAsset::new(faucet_a, 100000000000).unwrap().into();
    let asset_b_matcher: Asset = FungibleAsset::new(faucet_b, 100000000000).unwrap().into();

    // Create accounts
    let alice_account = mock_chain.add_pending_existing_wallet(Auth::BasicAuth, vec![]);
    let bob_account = mock_chain.add_pending_existing_wallet(Auth::BasicAuth, vec![]);
    let matcher_account = mock_chain
        .add_pending_existing_wallet(Auth::BasicAuth, vec![asset_a_matcher, asset_b_matcher]);

    // SWAPP NOTE 1: Alice offers 10 B, wants 45290 A (high price per A)
    let swap_note_1 = create_partial_swap_note(
        alice_account.id(),
        alice_account.id(),
        FungibleAsset::new(faucet_b, 10).unwrap().into(), // offered: 10 B
        FungibleAsset::new(faucet_a, 45290).unwrap().into(), // wanted: 45290 A
        Word::default(),
        0,
    )
    .unwrap();

    // SWAPP NOTE 2: Bob offers 54360 A, wants 12 B (low price per A - better deal)
    let swap_note_2 = create_partial_swap_note(
        bob_account.id(),
        bob_account.id(),
        FungibleAsset::new(faucet_a, 54360).unwrap().into(), // offered: 54360 A
        FungibleAsset::new(faucet_b, 12).unwrap().into(),    // wanted: 12 B
        Word::default(),
        0,
    )
    .unwrap();

    // Add notes to the chain
    let swapp_note1_output = OutputNote::Full(swap_note_1.clone());
    let swapp_note2_output = OutputNote::Full(swap_note_2.clone());

    mock_chain.add_pending_note(swapp_note1_output);
    mock_chain.add_pending_note(swapp_note2_output);
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
    let executed_transaction = mock_chain
        .build_tx_context(
            matcher_account.id(),
            &[swap_note_1.id(), swap_note_2.id()],
            &[],
        )?
        .extend_expected_output_notes(outputs)
        .build()?
        .execute()
        .await?;

    let final_matcher_account =
        mock_chain.add_pending_executed_transaction(&executed_transaction)?;

    // Verify the match was correct
    // Alice (note1) should get exactly 10 B (all she offered)
    assert_eq!(p2id_to_alice.amount(), 10, "Alice should receive 10 B");
    assert_eq!(
        p2id_to_alice.faucet_id(),
        faucet_b,
        "Alice should receive asset B"
    );

    // Bob (note2) should get exactly 45290 A (all Alice wanted)
    assert_eq!(p2id_to_bob.amount(), 45290, "Bob should receive 45290 A");
    assert_eq!(
        p2id_to_bob.faucet_id(),
        faucet_a,
        "Bob should receive asset A"
    );

    // There should be a leftover from Bob's order
    assert!(
        swap_data.leftover_swapp_note.is_some(),
        "Bob's order should be partially filled"
    );

    if let Some(ref leftover) = swap_data.leftover_swapp_note {
        let (offered, requested) = miden_clob::decompose_swapp_note(leftover).unwrap();
        // Bob originally offered 54360 A, gave away 45290 A, should have 9070 A left
        assert_eq!(offered.amount(), 9070, "Leftover should offer 9070 A");
        assert_eq!(offered.faucet_id(), faucet_a);
        // Bob originally wanted 12 B, received 10 B, should still want 2 B
        assert_eq!(requested.amount(), 2, "Leftover should want 2 B");
        assert_eq!(requested.faucet_id(), faucet_b);
    }

    println!("\n=== Test passed! ===");

    Ok(())
}
