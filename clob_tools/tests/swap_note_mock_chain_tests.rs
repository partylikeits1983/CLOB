use clob_tools::{create_partial_swap_note, create_inflight_partial_swap, try_match_swapp_notes};
use miden_client::{
    account::{AccountId, AccountStorageMode, AccountType},
    asset::{Asset, FungibleAsset},
    note::NoteType,
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1,
    transaction::OutputNote,
    Felt, Word,
};

use miden_objects::{
    account::AccountIdVersion, testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2,
};
use miden_testing::{Auth, MockChain};

#[test]
fn p2id_script_multiple_assets() {
    // Create assets
    let fungible_asset_1: Asset = FungibleAsset::mock(123);
    let fungible_asset_2: Asset =
        FungibleAsset::new(ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap(), 456)
            .unwrap()
            .into();

    // Create dummy account IDs
    let sender_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let target_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();

    // Create the note using the clob_tools function
    let note = clob_tools::create_p2id_note(
        sender_account_id,
        target_account_id,
        vec![fungible_asset_1, fungible_asset_2],
        NoteType::Public,
        Felt::new(0),
        [Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)],
    )
    .unwrap();

    println!("p2id script hash: {:?}", note.script().root());
}

#[tokio::test]
async fn swapp_match_mock_chain() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    let faucet_owner_account_id = AccountId::dummy(
        [1; 15],
        AccountIdVersion::Version0,
        AccountType::RegularAccountImmutableCode,
        AccountStorageMode::Private,
    );

    let faucet_1 =
        builder.add_existing_network_faucet("TOKA", 1000, faucet_owner_account_id, Some(100_000_000))?;

    let faucet_2 =
        builder.add_existing_network_faucet("TOKB", 1000, faucet_owner_account_id, Some(100_000_000))?;

    // matcher asset amounts
    let matcher_asset_a: Asset = FungibleAsset::new(faucet_1.id(), 1000).unwrap().into();
    let matcher_asset_b: Asset = FungibleAsset::new(faucet_2.id(), 1000).unwrap().into();

    // PSWAP NOTE 1
    let swap_note_1_asset_a: Asset = FungibleAsset::new(faucet_1.id(), 100).unwrap().into();
    let swap_note_1_asset_b: Asset = FungibleAsset::new(faucet_2.id(), 100).unwrap().into();

    let matcher_account = builder
        .add_existing_wallet_with_assets(Auth::BasicAuth, vec![matcher_asset_a, matcher_asset_b])?;

    // Create account IDs
    let alice_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let bob_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();
    let matcher_account_id = matcher_account.id();

    let swap_note_1 = create_partial_swap_note(
        alice_account_id,           // creator of the order
        alice_account_id,           // last account to "fill the order"
        swap_note_1_asset_a.into(), // offered asset (selling)
        swap_note_1_asset_b.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    // PSWAP NOTE 2
    let swap_note_2_asset_a: Asset = FungibleAsset::new(faucet_1.id(), 100).unwrap().into();
    let swap_note_2_asset_b: Asset = FungibleAsset::new(faucet_2.id(), 100).unwrap().into();

    let swap_note_2 = create_partial_swap_note(
        bob_account_id,             // creator of the order
        bob_account_id,             // last account to "fill the order"
        swap_note_2_asset_b.into(), // offered asset (selling)
        swap_note_2_asset_a.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    builder.add_output_note(OutputNote::Full(swap_note_1.clone()));
    builder.add_output_note(OutputNote::Full(swap_note_2.clone()));

    let mock_chain = builder.build()?;

    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account_id)
        .unwrap()
        .expect("orders should cross");

    let mut note_args = std::collections::BTreeMap::new();
    note_args.insert(swap_data.swap_note_1.id(), swap_data.note1_args.into());
    note_args.insert(swap_data.swap_note_2.id(), swap_data.note2_args.into());

    let tx_context_execute = mock_chain
        .build_tx_context(
            matcher_account.id(),
            &[swap_data.swap_note_1.id(), swap_data.swap_note_2.id()],
            &[],
        )?
        .extend_note_args(note_args)
        .extend_expected_output_notes(vec![
            // OutputNote::Full(swap_data.leftover_swapp_note.unwrap()),
            OutputNote::Full(swap_data.p2id_from_1_to_2),
            OutputNote::Full(swap_data.p2id_from_2_to_1),
        ])
        .build()?
        .execute()
        .await?;

    let status = tx_context_execute.account_delta();
    println!("status: {:?}", status);

    println!("cycles: {:?}", tx_context_execute.measurements().note_execution);
    Ok(())
}

#[tokio::test]
async fn in_flight_pswap_mockchain() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    let faucet_owner_account_id = AccountId::dummy(
        [1; 15],
        AccountIdVersion::Version0,
        AccountType::RegularAccountImmutableCode,
        AccountStorageMode::Private,
    );

    let faucet_1 =
        builder.add_existing_network_faucet("TOKA", 1000, faucet_owner_account_id, Some(100_000_000))?;

    let faucet_2 =
        builder.add_existing_network_faucet("TOKB", 1000, faucet_owner_account_id, Some(100_000_000))?;

    // matcher asset amounts
    let matcher_asset_a: Asset = FungibleAsset::new(faucet_1.id(), 1000).unwrap().into();
    let matcher_asset_b: Asset = FungibleAsset::new(faucet_2.id(), 1000).unwrap().into();

    // PSWAP NOTE 1
    let swap_note_1_asset_a: Asset = FungibleAsset::new(faucet_1.id(), 100).unwrap().into();
    let swap_note_1_asset_b: Asset = FungibleAsset::new(faucet_2.id(), 100).unwrap().into();

    let matcher_account = builder
        .add_existing_wallet_with_assets(Auth::BasicAuth, vec![matcher_asset_a, matcher_asset_b])?;

    // Create account IDs
    let alice_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let bob_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();
    let matcher_account_id = matcher_account.id();

    let swap_note_1 = create_inflight_partial_swap(
        alice_account_id,           // creator of the order
        alice_account_id,           // last account to "fill the order"
        swap_note_1_asset_a.into(), // offered asset (selling)
        swap_note_1_asset_b.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    // PSWAP NOTE 2
    let swap_note_2_asset_a: Asset = FungibleAsset::new(faucet_1.id(), 100).unwrap().into();
    let swap_note_2_asset_b: Asset = FungibleAsset::new(faucet_2.id(), 100).unwrap().into();

    let swap_note_2 = create_inflight_partial_swap(
        bob_account_id,             // creator of the order
        bob_account_id,             // last account to "fill the order"
        swap_note_2_asset_b.into(), // offered asset (selling)
        swap_note_2_asset_a.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    builder.add_output_note(OutputNote::Full(swap_note_1.clone()));
    builder.add_output_note(OutputNote::Full(swap_note_2.clone()));

    let mock_chain = builder.build()?;

    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account_id)
        .unwrap()
        .expect("orders should cross");

    let mut note_args = std::collections::BTreeMap::new();
    note_args.insert(swap_data.swap_note_1.id(), swap_data.note1_args.into());
    note_args.insert(swap_data.swap_note_2.id(), swap_data.note2_args.into());

    let tx_context_execute = mock_chain
        .build_tx_context(
            matcher_account.id(),
            &[swap_data.swap_note_1.id(), swap_data.swap_note_2.id()],
            &[],
        )?
        .extend_note_args(note_args)
        .extend_expected_output_notes(vec![
            // OutputNote::Full(swap_data.leftover_swapp_note.unwrap()),
            OutputNote::Full(swap_data.p2id_from_1_to_2),
            OutputNote::Full(swap_data.p2id_from_2_to_1),
        ])
        .build()?
        .execute()
        .await?;

    let status = tx_context_execute.account_delta();
    println!("status: {:?}", status);

    println!("cycles: {:?}", tx_context_execute.measurements().note_execution);
    Ok(())
}


#[tokio::test]
async fn swapp_match_mock_chain_exact_error_values() -> anyhow::Result<()> {
    // Test with the exact values from the error message:
    // Note 1: offers 10, wants 45290
    // Note 2: offers 54360, wants 12

    // Create faucets for the two assets
    let faucet_a = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let faucet_b = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();

    // Initialize assets for the matcher account (needs enough to cover both sides)
    let _asset_a_matcher: Asset = FungibleAsset::new(faucet_a, 100000000000).unwrap().into();
    let _asset_b_matcher: Asset = FungibleAsset::new(faucet_b, 100000000000).unwrap().into();

    // Create account IDs
    let alice_account_id = faucet_a;
    let bob_account_id = faucet_b;
    let matcher_account_id = faucet_a;

    // PSWAP NOTE 1: Alice offers 10 B, wants 45290 A (high price per A)
    let swap_note_1 = create_partial_swap_note(
        alice_account_id,
        alice_account_id,
        FungibleAsset::new(faucet_b, 10).unwrap().into(), // offered: 10 B
        FungibleAsset::new(faucet_a, 45290).unwrap().into(), // wanted: 45290 A
        *Word::default(),
        0,
    )
    .unwrap();

    // PSWAP NOTE 2: Bob offers 54360 A, wants 12 B (low price per A - better deal)
    let swap_note_2 = create_partial_swap_note(
        bob_account_id,
        bob_account_id,
        FungibleAsset::new(faucet_a, 54360).unwrap().into(), // offered: 54360 A
        FungibleAsset::new(faucet_b, 12).unwrap().into(),    // wanted: 12 B
        *Word::default(),
        0,
    )
    .unwrap();

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
    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account_id)
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
        let (offered, requested) = clob_tools::decompose_swapp_note(leftover).unwrap();
        println!("Leftover PSWAP note:");
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
        println!("No leftover PSWAP note (complete fill)");
    }

    // There should be a leftover from Bob's order
    assert!(
        swap_data.leftover_swapp_note.is_some(),
        "Bob's order should be partially filled"
    );

    println!("\n=== Test passed! ===");

    Ok(())
}
