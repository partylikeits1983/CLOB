use clob_tools::create_arbint_note;
// use miden_client::crypto::rpo_falcon512::PublicKey;
// use miden_crypto::hash::rpo::Rpo256;
use miden_objects::account::auth::AuthSecretKey;
use miden_objects::{
    account::{AccountId, AccountIdVersion, AccountStorageMode, AccountType},
    asset::{Asset, FungibleAsset},
    note::NoteType,
    transaction::OutputNote,
    Felt, Hasher, Word,
};
use miden_testing::{Auth, MockChain};
use miden_tx::auth::{BasicAuthenticator, SigningInputs, TransactionAuthenticator};
// use miden_tx::TransactionExecutorError;
// use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

/// Tests the ARBINT note with signature-based flow similar to multisig.
///
/// This test implements the desired flow:
/// 1. Alice creates an ARBINT note with a signed value (100)
/// 2. Alice signs the value and the signature is used for verification
/// 3. Bob consumes the ARBINT note using Alice's signature
///
/// **Roles:**
/// - Alice: Creates the note and signs a value
/// - Bob: Consumes the note using Alice's signature
#[tokio::test]
async fn test_arbint_note_with_signature_flow() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();
    let seed: [u8; 32] = rand::random();
    let mut rng = ChaCha20Rng::from_seed(seed);

    let alice_secret_key = AuthSecretKey::new_rpo_falcon512_with_rng(&mut rng);
    let alice_public_key: miden_client::auth::PublicKey = alice_secret_key.public_key();
    let alice_authenticator = BasicAuthenticator::new(core::slice::from_ref(&alice_secret_key));

    let faucet_owner_account_id = AccountId::dummy(
        [1; 15],
        AccountIdVersion::Version0,
        AccountType::RegularAccountImmutableCode,
        AccountStorageMode::Private,
    );

    // Create network faucets to provide assets
    let faucet_1 =
        builder.add_existing_network_faucet("ARBA", 1000, faucet_owner_account_id, Some(500))?;

    let amount_1 = Felt::new(100);
    let alice_asset_1: Asset = FungibleAsset::new(faucet_1.id(), amount_1.into())
        .unwrap()
        .into();

    let alice_account =
        builder.add_existing_wallet_with_assets(Auth::BasicAuth, vec![alice_asset_1])?;

    // Create Bob's account (will consume the ARBINT note)
    let mut bob_account = builder.add_existing_wallet(Auth::BasicAuth)?;

    // ALICE CREATES ARBINT NOTE WITH SIGNED VALUE
    // --------------------------------------------------------------------------------------------

    // Create the ARBINT note using the existing function
    // Alice is sending her assets to Bob via the ARBINT note
    let arbint_note = create_arbint_note(
        alice_public_key.clone(),
        alice_account.id(),
        vec![alice_asset_1],
        NoteType::Public,
        Felt::new(0),
        [Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)],
    )?;

    // Add the ARBINT note to the mock chain
    builder.add_output_note(OutputNote::Full(arbint_note.clone()));
    let mut mock_chain = builder.build()?;

    // Store the initial asset balances of Bob's account
    let initial_balance_1 = bob_account.vault().get_balance(faucet_1.id()).unwrap();

    // ALICE SIGNS THE VALUE
    // --------------------------------------------------------------------------------------------
    let signed_value = Felt::new(100);
    let arbitrary_inputs = SigningInputs::Arbitrary(vec![signed_value]);
    let msg: Word = arbitrary_inputs.to_commitment();

    let alice_signature = alice_authenticator
        .get_signature(alice_public_key.to_commitment(), &arbitrary_inputs)
        .await?;

    let is_valid = alice_public_key.verify(msg, alice_signature.clone());
    println!("isvalid: {:?}", is_valid);

    // Create the message hash for the signature (hash of the signed value)
    println!("public key: {:?}", alice_public_key.to_commitment());
    println!("msg: {:?}", msg);

    // BOB CONSUMES THE ARBINT NOTE USING ALICE'S SIGNATURE
    // --------------------------------------------------------------------------------------------

    println!("_______________________________________________________\n");
    println!("Step 3: CONSUMING ARBINT note using Alice's signature");

    // Create note args with the message hash
    let mut note_args = std::collections::BTreeMap::new();
    note_args.insert(arbint_note.id(), msg.into());

    let tx_context_execute = mock_chain
        .build_tx_context(bob_account.id(), &[arbint_note.id()], &[])?
        .extend_note_args(note_args)
        .add_signature(
            alice_public_key.to_commitment(),
            msg.into(),
            alice_signature,
        )
        .build()?
        .execute()
        .await?;

    let mut advstack = tx_context_execute.tx_inputs().advice_inputs().map.clone();

    println!(
        "advstack: {:?}",
        advstack.entry(Hasher::merge(&[
            alice_public_key.clone().to_commitment().into(),
            msg.clone()
        ]))
    );

    // VERIFY NO OUTPUT NOTES WERE CREATED (ASSETS ADDED TO ACCOUNT)
    // --------------------------------------------------------------------------------------------
    // The ARBINT note should simply add assets to Bob's account
    assert_eq!(
        tx_context_execute.output_notes().num_notes(),
        0,
        "ARBINT note consumption should not create output notes"
    );

    // Apply the delta to Bob's account
    bob_account.apply_delta(tx_context_execute.account_delta())?;

    // VERIFY ASSETS WERE ADDED TO BOB'S ACCOUNT
    // --------------------------------------------------------------------------------------------
    let final_balance_1 = bob_account
        .vault()
        .get_balance(faucet_1.id())
        .unwrap_or(0u64);

    let expected_balance_1 = initial_balance_1 + amount_1.as_int();

    assert_eq!(
        final_balance_1, expected_balance_1,
        "Bob should have received asset 1 from the ARBINT note"
    );

    mock_chain.add_pending_executed_transaction(&tx_context_execute)?;
    mock_chain.prove_next_block()?;

    Ok(())
}
