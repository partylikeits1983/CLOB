use clob_tools::{create_partial_swap_note, try_match_swapp_notes};
use miden_client::{
    asset::{Asset, FungibleAsset},
    note::NoteType,
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1,
    Felt, Word,
};

use miden_objects::testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2;

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
    // Create account IDs
    let alice_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();
    let bob_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2.try_into().unwrap();
    let matcher_account_id = ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1.try_into().unwrap();

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
        alice_account_id,           // creator of the order
        alice_account_id,           // last account to "fill the order"
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
        bob_account_id,             // creator of the order
        bob_account_id,             // last account to "fill the order"
        swap_note_2_asset_b.into(), // offered asset (selling)
        swap_note_2_asset_a.into(), // requested asset (buying)
        *Word::default(),           // serial number of the order
        0,                          // fill number (0 means hasn't been filled)
    )
    .unwrap();

    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_account_id)
        .unwrap()
        .expect("orders should cross");

    println!("Match successful - test passed!");

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

    // SWAPP NOTE 1: Alice offers 10 B, wants 45290 A (high price per A)
    let swap_note_1 = create_partial_swap_note(
        alice_account_id,
        alice_account_id,
        FungibleAsset::new(faucet_b, 10).unwrap().into(), // offered: 10 B
        FungibleAsset::new(faucet_a, 45290).unwrap().into(), // wanted: 45290 A
        *Word::default(),
        0,
    )
    .unwrap();

    // SWAPP NOTE 2: Bob offers 54360 A, wants 12 B (low price per A - better deal)
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

    // There should be a leftover from Bob's order
    assert!(
        swap_data.leftover_swapp_note.is_some(),
        "Bob's order should be partially filled"
    );

    println!("\n=== Test passed! ===");

    Ok(())
}
