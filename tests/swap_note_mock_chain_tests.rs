use std::{fs, path::Path};

use miden_client::{
    account::{Account, AccountId},
    asset::{Asset, AssetVault, FungibleAsset},
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteInputs, NoteMetadata,
        NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_1,
    transaction::TransactionRequestBuilder,
    Felt, Word,
};
use miden_clob::{create_partial_swap_note, try_match_swapp_notes};
use miden_lib::transaction::TransactionKernel;
use miden_testing::{Auth, MockChain};

use miden_objects::{
    note::NoteDetails, testing::account_id::ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2,
    transaction::OutputNote,
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
