use std::time::Instant;

use miden_client::{
    ClientError, Felt,
    asset::{Asset, FungibleAsset},
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::NoteType,
    rpc::{Endpoint, TonicRpcClient},
    transaction::{OutputNote, TransactionRequestBuilder},
};
use miden_testing::{Auth, MockChain};

use miden_objects::testing::account_id::{
    ACCOUNT_ID_PRIVATE_FUNGIBLE_FAUCET, ACCOUNT_ID_PUBLIC_FUNGIBLE_FAUCET_2,
    ACCOUNT_ID_REGULAR_PUBLIC_ACCOUNT_IMMUTABLE_CODE,
    ACCOUNT_ID_REGULAR_PUBLIC_ACCOUNT_IMMUTABLE_CODE_2, ACCOUNT_ID_SENDER,
};

use miden_clob::{
    common::{
        compute_partial_swapp, create_order, create_p2id_note, create_partial_swap_note,
        delete_keystore_and_store, get_p2id_serial_num, get_swapp_note, instantiate_client,
        setup_accounts_and_faucets, try_match_swapp_notes, wait_for_note,
    },
    create_order_simple,
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
            None,
        )
        .unwrap();

    mock_chain.prove_next_block();

    println!("p2id script hash: {:?}", note.script().root());
}
