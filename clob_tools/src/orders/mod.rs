use miden_client::{
    account::AccountId,
    asset::{Asset, FungibleAsset},
    crypto::FeltRng,
    keystore::FilesystemKeyStore,
    note::Note,
    transaction::{OutputNote, TransactionRequestBuilder},
    Client, Felt, Word,
};
use miden_objects::NoteError;
use rand::rngs::StdRng;

pub async fn create_order(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    trader: AccountId,
    buy_asset: Asset,
    sell_asset: Asset,
) -> Result<Note, NoteError> {
    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = crate::swap::operations::create_partial_swap_note(
        trader,
        trader,
        sell_asset.into(),
        buy_asset.into(),
        *swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();

    let tx_id = client
        .submit_new_transaction(trader, note_req)
        .await
        .unwrap();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    client.sync_state().await.unwrap();

    Ok(swapp_note)
}

pub async fn create_order_simple(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    trader: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
) -> Result<Note, NoteError> {
    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = crate::swap::operations::create_partial_swap_note(
        trader,
        trader,
        offered_asset.into(),
        requested_asset.into(),
        *swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();

    let tx_id = client
        .submit_new_transaction(trader, note_req)
        .await
        .unwrap();
    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_id
    );

    client.sync_state().await.unwrap();

    Ok(swapp_note)
}

pub fn create_order_simple_testing(
    trader: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
) -> Note {
    let swap_serial_num = Word::default();
    let swap_count = 0;

    let swapp_note = crate::swap::operations::create_partial_swap_note(
        trader,
        trader,
        offered_asset.into(),
        requested_asset.into(),
        *swap_serial_num,
        swap_count,
    )
    .unwrap();

    swapp_note
}

/// Helper — create a partial swap note from a price.
/// If is_bid is false (selling): offers quantity of faucet_a, wants quantity * price of faucet_b
/// If is_bid is true (buying): offers quantity * price of faucet_b, wants quantity of faucet_a
pub fn price_to_swap_note(
    creator: AccountId,
    last_filler: AccountId,
    is_bid: bool,         // true = bid (buying faucet_a), false = ask (selling faucet_a)
    price: u64,           // price in units of faucet_b per unit of faucet_a
    quantity: u64,        // quantity of faucet_a to buy/sell
    faucet_a: &AccountId, // base asset
    faucet_b: &AccountId, // quote asset
    serial: [Felt; 4],
) -> Note {
    let (offered, requested) = if is_bid {
        // Buying: offer quantity * price of faucet_b, want quantity of faucet_a
        (
            FungibleAsset::new(*faucet_b, quantity * price).unwrap(),
            FungibleAsset::new(*faucet_a, quantity).unwrap(),
        )
    } else {
        // Selling: offer quantity of faucet_a, want quantity * price of faucet_b
        (
            FungibleAsset::new(*faucet_a, quantity).unwrap(),
            FungibleAsset::new(*faucet_b, quantity * price).unwrap(),
        )
    };

    crate::swap::operations::create_partial_swap_note(
        creator,     // creator
        last_filler, // initially the same account
        offered.into(),
        requested.into(),
        serial,
        0, // not filled yet
    )
    .unwrap()
}
