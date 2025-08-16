use miden_assembly::{
    ast::{Module, ModuleKind},
    Assembler, DefaultSourceManager, LibraryPath,
};
use rand::{rngs::StdRng, RngCore};
use std::{env, fmt, fs, path::PathBuf, sync::Arc};
use tokio::time::{sleep, Duration};

use miden_client::{
    account::{
        component::{BasicFungibleFaucet, BasicWallet, RpoFalcon512},
        Account, AccountBuilder, AccountId, AccountStorageMode, AccountType, StorageSlot,
    },
    asset::{Asset, FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    builder::ClientBuilder,
    crypto::{FeltRng, SecretKey},
    keystore::FilesystemKeyStore,
    note::{
        build_swap_tag, Note, NoteAssets, NoteExecutionHint, NoteId, NoteInputs, NoteMetadata,
        NoteRecipient, NoteRelevance, NoteScript, NoteTag, NoteType,
    },
    rpc::{Endpoint, TonicRpcClient},
    store::InputNoteRecord,
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder},
    Client, ClientError, Felt, Word,
};
use miden_lib::account::auth;

use miden_objects::{account::AccountComponent, Hasher, NoteError};
use serde::de::value::Error;

pub fn create_library(
    assembler: Assembler,
    library_path: &str,
    source_code: &str,
) -> Result<miden_assembly::Library, Box<dyn std::error::Error>> {
    let source_manager = Arc::new(DefaultSourceManager::default());
    let module = Module::parser(ModuleKind::Library).parse_str(
        LibraryPath::new(library_path)?,
        source_code,
        &source_manager,
    )?;
    let library = assembler.clone().assemble_library([module])?;
    Ok(library)
}

pub async fn create_basic_account(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<(miden_client::account::Account, SecretKey), ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = SecretKey::with_rng(client.rng());
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Private)
        .with_auth_component(RpoFalcon512::new(key_pair.public_key().clone()))
        .with_component(BasicWallet);
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair.clone()))
        .unwrap();

    Ok((account, key_pair))
}

// TODO: Currently faucets are setup with `NoAuth` auth component
pub async fn create_basic_faucet(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = SecretKey::with_rng(client.rng());
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000_000);
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(auth::NoAuth)
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap());
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair))
        .unwrap();
    Ok(account)
}

/// Creates [num_accounts] accounts, [num_faucets] faucets, and mints the given [balances].
///
/// - `balances[a][f]`: how many tokens faucet `f` should mint for account `a`.
/// - Returns: a tuple of `(Vec<Account>, Vec<Account>)` i.e. (accounts, faucets).
pub async fn setup_accounts_and_faucets(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
    num_accounts: usize,
    num_faucets: usize,
    balances: Vec<Vec<u64>>,
) -> Result<(Vec<Account>, Vec<Account>), ClientError> {
    // ---------------------------------------------------------------------
    // 1)  Create basic accounts
    // ---------------------------------------------------------------------
    let mut accounts = Vec::with_capacity(num_accounts);
    for i in 0..num_accounts {
        let (account, _) = create_basic_account(client, keystore.clone()).await?;
        println!("Created Account #{i} ⇒ ID: {:?}", account.id().to_hex());
        accounts.push(account);
    }

    // ---------------------------------------------------------------------
    // 2)  Create basic faucets
    // ---------------------------------------------------------------------
    let mut faucets = Vec::with_capacity(num_faucets);
    for j in 0..num_faucets {
        let faucet = create_basic_faucet(client, keystore.clone()).await?;
        println!("Created Faucet #{j} ⇒ ID: {:?}", faucet.id().to_hex());
        faucets.push(faucet);
    }

    // Tell the client about the new accounts/faucets
    client.sync_state().await?;

    // ---------------------------------------------------------------------
    // 3)  Mint tokens
    // ---------------------------------------------------------------------
    // `minted_notes[i]` collects the notes minted **for** `accounts[i]`
    let mut minted_notes: Vec<Vec<Note>> = vec![Vec::new(); num_accounts];

    for (acct_idx, account) in accounts.iter().enumerate() {
        for (faucet_idx, faucet) in faucets.iter().enumerate() {
            let amount = balances[acct_idx][faucet_idx];
            if amount == 0 {
                continue;
            }

            println!("Minting {amount} tokens from Faucet #{faucet_idx} to Account #{acct_idx}");

            // Build & submit the mint transaction
            let asset = FungibleAsset::new(faucet.id(), amount).unwrap();
            let tx_request = TransactionRequestBuilder::new()
                .build_mint_fungible_asset(asset, account.id(), NoteType::Public, client.rng())
                .unwrap();

            let tx_exec = client.new_transaction(faucet.id(), tx_request).await?;
            client.submit_transaction(tx_exec.clone()).await?;

            // Remember the freshly-created note so we can consume it later
            let minted_note = match tx_exec.created_notes().get_note(0) {
                OutputNote::Full(n) => n.clone(),
                _ => panic!("Expected OutputNote::Full, got something else"),
            };
            minted_notes[acct_idx].push(minted_note);
        }
    }

    // ---------------------------------------------------------------------
    // 4)  ONE wait-phase – ensure every account can now see all its notes
    // ---------------------------------------------------------------------
    for (acct_idx, account) in accounts.iter().enumerate() {
        let expected = minted_notes[acct_idx].len();
        if expected > 0 {
            wait_for_notes(client, account, expected).await?;
        }
    }
    client.sync_state().await?;

    // ---------------------------------------------------------------------
    // 5)  Consume notes so the tokens live in the public vaults
    // ---------------------------------------------------------------------
    for (acct_idx, account) in accounts.iter().enumerate() {
        for note in &minted_notes[acct_idx] {
            let consume_req = TransactionRequestBuilder::new()
                .authenticated_input_notes([(note.id(), None)])
                .build()
                .unwrap();

            let tx_exec = client.new_transaction(account.id(), consume_req).await?;
            client.submit_transaction(tx_exec).await?;
        }
    }
    client.sync_state().await?;

    Ok((accounts, faucets))
}

pub async fn wait_for_notes(
    client: &mut Client,
    account_id: &miden_client::account::Account,
    expected: usize,
) -> Result<(), ClientError> {
    loop {
        client.sync_state().await?;
        let notes = client.get_consumable_notes(Some(account_id.id())).await?;
        if notes.len() >= expected {
            break;
        }
        println!(
            "{} consumable notes found for account {}. Waiting...",
            notes.len(),
            account_id.id().to_hex()
        );
        sleep(Duration::from_secs(3)).await;
    }
    Ok(())
}

pub async fn get_swapp_note(
    client: &mut Client,
    tag: NoteTag,
    swapp_note_id: NoteId,
) -> Result<(), ClientError> {
    loop {
        // Sync the state and add the tag
        client.sync_state().await?;
        client.add_note_tag(tag).await?;

        // Fetch notes
        let notes = client.get_consumable_notes(None).await?;

        // Check if any note matches the swapp_note_id
        let found = notes.iter().any(|(note, _)| note.id() == swapp_note_id);

        if found {
            println!("Found the note with ID: {:?}", swapp_note_id);
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}

pub fn create_partial_swap_note(
    creator: AccountId,
    last_consumer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    swap_serial_num: [Felt; 4],
    swap_count: u64,
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "SWAPP.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Public;

    let requested_asset_word: Word = requested_asset.into();
    let swapp_tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let p2id_tag = NoteTag::from_account_id(creator);

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.into(),
        p2id_tag.into(),
        Felt::new(0),
        Felt::new(0),
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        creator.prefix().into(),
        creator.suffix().into(),
    ])?;

    let aux = Felt::new(0);

    // build the outgoing note
    let metadata = NoteMetadata::new(
        last_consumer,
        note_type,
        swapp_tag,
        NoteExecutionHint::always(),
        aux,
    )?;

    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(swap_serial_num, note_script.clone(), inputs.clone());
    let note = Note::new(assets.clone(), metadata, recipient.clone());

    Ok(note)
}

pub fn create_partial_swap_note_cancellable(
    creator: AccountId,
    last_consumer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    secret_hash: [Felt; 4],
    swap_serial_num: [Felt; 4],
    swap_count: u64,
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "SWAPP_cancellable.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Public;

    let requested_asset_word: Word = requested_asset.into();
    let swapp_tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let p2id_tag = NoteTag::from_account_id(creator);

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.into(),
        p2id_tag.into(),
        Felt::new(0),
        Felt::new(0),
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        creator.prefix().into(),
        creator.suffix().into(),
        Felt::new(0),
        Felt::new(0),
        secret_hash[0],
        secret_hash[1],
        secret_hash[2],
        secret_hash[3],
    ])?;

    let aux = Felt::new(0);

    // build the outgoing note
    let metadata = NoteMetadata::new(
        last_consumer,
        note_type,
        swapp_tag,
        NoteExecutionHint::always(),
        aux,
    )?;

    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(swap_serial_num, note_script.clone(), inputs.clone());
    let note = Note::new(assets.clone(), metadata, recipient.clone());

    println!(
        "inputlen: {:?}, NoteInputs: {:?}",
        inputs.num_values(),
        inputs.values()
    );
    println!("tag: {:?}", note.metadata().tag());
    println!("aux: {:?}", note.metadata().aux());
    println!("note type: {:?}", note.metadata().note_type());
    println!("hint: {:?}", note.metadata().execution_hint());
    println!("recipient: {:?}", note.recipient().digest());

    Ok(note)
}

pub async fn create_order(
    client: &mut Client,
    trader: AccountId,
    buy_asset: Asset,
    sell_asset: Asset,
) -> Result<Note, NoteError> {
    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        trader,
        trader,
        sell_asset.into(),
        buy_asset.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client.new_transaction(trader, note_req).await.unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await.unwrap();

    Ok(swapp_note)
}

pub async fn create_order_simple(
    client: &mut Client,
    trader: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
) -> Result<Note, NoteError> {
    let swap_serial_num = client.rng().draw_word();
    let swap_count = 0;

    let swapp_note = create_partial_swap_note(
        trader,
        trader,
        offered_asset.into(),
        requested_asset.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    let note_req = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
        .build()
        .unwrap();
    let tx_result = client.new_transaction(trader, note_req).await.unwrap();

    println!(
        "View transaction on MidenScan: https://testnet.midenscan.com/tx/{:?}",
        tx_result.executed_transaction().id()
    );

    let _ = client.submit_transaction(tx_result).await;
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

    let swapp_note = create_partial_swap_note(
        trader,
        trader,
        offered_asset.into(),
        requested_asset.into(),
        swap_serial_num,
        swap_count,
    )
    .unwrap();

    swapp_note
}

pub fn create_p2id_note(
    sender: AccountId,
    target: AccountId,
    assets: Vec<Asset>,
    note_type: NoteType,
    aux: Felt,
    serial_num: [Felt; 4],
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "P2ID.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let assembler = TransactionKernel::assembler().with_debug_mode(true);

    let note_script = NoteScript::compile(note_code, assembler).unwrap();

    let inputs = NoteInputs::new(vec![target.suffix(), target.prefix().into()])?;
    let tag = NoteTag::from_account_id(target);

    let metadata = NoteMetadata::new(sender, note_type, tag, NoteExecutionHint::always(), aux)?;
    let vault = NoteAssets::new(assets)?;

    let recipient = NoteRecipient::new(serial_num, note_script, inputs.clone());

    Ok(Note::new(vault, metadata, recipient))
}

pub async fn delete_keystore_and_store() {
    // Remove the SQLite store file
    let store_path = "./store.sqlite3";
    if tokio::fs::metadata(store_path).await.is_ok() {
        if let Err(e) = tokio::fs::remove_file(store_path).await {
            eprintln!("failed to remove {}: {}", store_path, e);
        } else {
            println!("cleared sqlite store: {}", store_path);
        }
    } else {
        println!("store not found: {}", store_path);
    }

    // Remove all files in the ./keystore directory
    let keystore_dir = "./keystore";
    match tokio::fs::read_dir(keystore_dir).await {
        Ok(mut dir) => {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let file_path = entry.path();
                if let Err(e) = tokio::fs::remove_file(&file_path).await {
                    eprintln!("failed to remove {}: {}", file_path.display(), e);
                } else {
                    println!("removed file: {}", file_path.display());
                }
            }
        }
        Err(e) => eprintln!("failed to read directory {}: {}", keystore_dir, e),
    }
}

pub fn get_p2id_serial_num(swap_serial_num: [Felt; 4], swap_count: u64) -> [Felt; 4] {
    let swap_count_word = [
        Felt::new(swap_count),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
    ];
    let p2id_serial_num = Hasher::merge(&[swap_serial_num.into(), swap_count_word.into()]);

    p2id_serial_num.into()
}

pub fn create_option_contract_note<R: FeltRng>(
    underwriter: AccountId,
    buyer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    expiration: u64,
    is_european: bool,
    aux: Felt,
    rng: &mut R,
) -> Result<(Note, Note), NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "notes", "option_contract_note.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));
    let assembler = TransactionKernel::assembler().with_debug_mode(true);
    let note_script = NoteScript::compile(note_code, assembler).unwrap();
    let note_type = NoteType::Public;

    let payback_serial_num = rng.draw_word();
    let p2id_note = create_p2id_note(
        buyer,
        underwriter,
        vec![requested_asset.into()],
        NoteType::Public,
        Felt::new(0),
        payback_serial_num,
    )
    .unwrap();

    let payback_recipient_word: Word = p2id_note.recipient().digest().into();
    let requested_asset_word: Word = requested_asset.into();
    let payback_tag = NoteTag::from_account_id(underwriter);

    let inputs = NoteInputs::new(vec![
        payback_recipient_word[0],
        payback_recipient_word[1],
        payback_recipient_word[2],
        payback_recipient_word[3],
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        payback_tag.into(),
        NoteExecutionHint::always().into(),
        underwriter.prefix().into(),
        underwriter.suffix().into(),
        buyer.prefix().into(),
        buyer.suffix(),
        Felt::new(expiration),
        Felt::new(is_european as u64),
    ])?;

    // build the tag for the SWAP use case
    let tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let serial_num = rng.draw_word();

    // build the outgoing note
    let metadata = NoteMetadata::new(
        underwriter,
        note_type,
        tag,
        NoteExecutionHint::always(),
        aux,
    )?;
    let assets = NoteAssets::new(vec![offered_asset])?;
    let recipient = NoteRecipient::new(serial_num, note_script, inputs);
    let note = Note::new(assets, metadata, recipient);

    Ok((note, p2id_note))
}

/// Computes how many of the offered asset go out given `requested_asset_filled`,
/// then returns both the partial-fill amounts and the new remaining amounts.
///
/// Formulas:
///   amount_out = (offered_swapp_asset_amount * requested_asset_filled)
///                / requested_swapp_asset_amount
///
///   new_offered_asset_amount = offered_swapp_asset_amount - amount_out
///
///   new_requested_asset_amount = requested_swapp_asset_amount - requested_asset_filled
///
/// Returns a tuple of:
/// (amount_out, requested_asset_filled, new_offered_asset_amount, new_requested_asset_amount)
/// where:
///   - `amount_out` is how many of the offered asset will be sent out,
///   - `requested_asset_filled` is how many of the requested asset the filler provides,
///   - `new_offered_asset_amount` is how many of the offered asset remain unfilled,
///   - `new_requested_asset_amount` is how many of the requested asset remain unfilled.
pub fn compute_partial_swapp(
    offered_swapp_asset_amount: u64,
    requested_swapp_asset_amount: u64,
    requested_asset_filled: u64,
) -> (u64, u64, u64) {
    // amount of "offered" tokens (A) to send out
    let mut amount_out_offered = offered_swapp_asset_amount
        .saturating_mul(requested_asset_filled)
        .saturating_div(requested_swapp_asset_amount);

    // update leftover offered amount
    let new_offered_asset_amount = offered_swapp_asset_amount.saturating_sub(amount_out_offered);

    if amount_out_offered > offered_swapp_asset_amount {
        amount_out_offered = offered_swapp_asset_amount;
    }

    // update leftover requested amount
    let new_requested_asset_amount =
        requested_swapp_asset_amount.saturating_sub(requested_asset_filled);

    // Return partial fill info and updated amounts
    (
        amount_out_offered,
        new_offered_asset_amount,
        new_requested_asset_amount,
    )
}

// Helper to instantiate Client
pub async fn instantiate_client(endpoint: Endpoint) -> Result<Client, ClientError> {
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));

    let client = ClientBuilder::new()
        .rpc(rpc_api.clone())
        .filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    Ok(client)
}

// Contract builder helper function
pub async fn create_public_immutable_contract(
    account_code: &String,
) -> Result<(Account, Word), ClientError> {
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    let counter_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![StorageSlot::Value([
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
        ])],
    )
    .unwrap()
    .with_supports_all_types();

    // @dev this is bad that I need to get an anchor block to create a contract
    let endpoint: Endpoint =
        Endpoint::try_from(env::var("MIDEN_NODE_ENDPOINT").unwrap().as_str()).unwrap();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));
    let mut client = ClientBuilder::new()
        .rpc(rpc_api.clone())
        .filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let (counter_contract, counter_seed) = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(counter_component.clone())
        .with_component(BasicWallet)
        .build()
        .unwrap();

    Ok((counter_contract, counter_seed))
}

// Waits for note
pub async fn wait_for_note(
    client: &mut Client,
    _account_id: &Account,
    expected: &Note,
) -> Result<(), ClientError> {
    loop {
        client.sync_state().await?;

        // let res = client.get_note_consumability()

        let notes: Vec<(InputNoteRecord, Vec<(AccountId, NoteRelevance)>)> =
            client.get_consumable_notes(None).await?;

        let found = notes.iter().any(|(rec, _)| rec.id() == expected.id());

        if found {
            println!("✅ note found {}", expected.id().to_hex());
            break;
        }

        println!("Note {} not found. Waiting...", expected.id().to_hex());
        sleep(Duration::from_secs(3)).await;
    }
    Ok(())
}

/// Matching SWAPP notes

// --------------------------------------------------------------------------
// Helper functions
// --------------------------------------------------------------------------

// Returns offered & requested assets
pub fn decompose_swapp_note(note: &Note) -> Result<(FungibleAsset, FungibleAsset), Error> {
    let offered_asset = note
        .assets()
        .iter()
        .next()
        .expect("note has no assets")
        .unwrap_fungible();

    let note_inputs: &[Felt] = note.inputs().values();
    let requested: &[Felt] = note_inputs.get(..4).expect("note has fewer than 4 inputs");

    let requested_id = AccountId::try_from([requested[3], requested[2]]).unwrap();
    let requested_asset = FungibleAsset::new(requested_id, requested[0].as_int()).unwrap();

    Ok((offered_asset, requested_asset))
}

/// Convenience: creator = first two field elements in the inputs after the
/// requested asset word.
/// (Exactly how SWAPP.masm constructs it.)
pub fn creator_of(note: &Note) -> AccountId {
    let vals = note.inputs().values();
    let prefix = Felt::from(vals[12]);
    let suffix = Felt::from(vals[13]);

    let account_id = AccountId::try_from([prefix, suffix]).unwrap();

    account_id
}

/// Three notes are produced when the maker (‖note 1‖) is only *partially*
/// filled; otherwise the SWAPP note is `None` and only the two P2ID notes
/// are returned.

/// Everything the matcher needs in order to build a single
/// consume-transaction that crosses the two SWAPP orders.
#[derive(Clone)]
pub struct MatchedSwap {
    /// P2ID note that transfers the *base* asset
    ///   maker → taker (created by the matcher).
    pub p2id_from_1_to_2: Note,

    /// P2ID note that transfers the *quote* asset
    ///   taker → maker (created by the matcher).
    pub p2id_from_2_to_1: Note,

    /// Remaining piece of the maker’s order, if it was not filled
    /// completely. `None` means the maker was filled in full.
    pub leftover_swapp_note: Option<Note>,

    // Input Note 1
    pub swap_note_1: Note,

    // Input Note 2
    pub swap_note_2: Note,

    /// `note_args` that **must** be supplied when the matcher consumes
    /// *maker*’s SWAPP note (`note1`).
    pub note1_args: [Felt; 4],

    /// `note_args` that **must** be supplied when the matcher consumes
    /// *taker*’s SWAPP note (`note2`).
    pub note2_args: [Felt; 4],
}

impl fmt::Debug for MatchedSwap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // ────────────────────────────────────────────────────────────────
        // helpers
        // ────────────────────────────────────────────────────────────────
        fn assets_str(note: &Note) -> String {
            note.assets()
                .iter()
                .map(|a| {
                    let f: FungibleAsset = a.unwrap_fungible();
                    format!("{} × {}", f.amount(), f.faucet_id())
                })
                .collect::<Vec<_>>()
                .join(", ")
        }

        fn swapp_str(note: &Note) -> String {
            match decompose_swapp_note(note) {
                Ok((off, req)) => format!(
                    "[offered: {} × {} → requested: {} × {}]",
                    off.amount(),
                    off.faucet_id(),
                    req.amount(),
                    req.faucet_id()
                ),
                Err(_) => "<cannot decode swapp note>".into(),
            }
        }

        // ────────────────────────────────────────────────────────────────
        // header lines requested
        // ────────────────────────────────────────────────────────────────
        writeln!(
            f,
            "swap note 1 serial num: {:?}",
            self.swap_note_1.serial_num()
        )?;
        writeln!(
            f,
            "swap note 2 serial num: {:?}",
            self.swap_note_2.serial_num()
        )?;
        if let Some(note) = &self.leftover_swapp_note {
            writeln!(
                f,
                "leftover swap digest: {}",
                note.recipient().digest().to_hex()
            )?;
        } else {
            writeln!(f, "None")?;
        }

        // ────────────────────────────────────────────────────────────────
        // original structured debug info
        // ────────────────────────────────────────────────────────────────
        let p2id_1 = format!("[assets: {}]", assets_str(&self.p2id_from_1_to_2));
        let p2id_2 = format!("[assets: {}]", assets_str(&self.p2id_from_2_to_1));
        let swapp = self.leftover_swapp_note.as_ref().map(swapp_str);

        f.debug_struct("MatchedSwap")
            .field("p2id_from_1_to_2", &p2id_1)
            .field("p2id_from_2_to_1", &p2id_2)
            .field("leftover_swapp_note", &swapp)
            .finish()
    }
}

pub fn try_match_swapp_notes(
    note1_in: &Note,
    note2_in: &Note,
    matcher: AccountId,
) -> Result<Option<MatchedSwap>, Error> {
    let (offer1_raw, want1_raw) = decompose_swapp_note(note1_in)?;
    let (offer2_raw, want2_raw) = decompose_swapp_note(note2_in)?;

    // 1. must be matchable
    if offer1_raw.faucet_id() != want2_raw.faucet_id()
        || want1_raw.faucet_id() != offer2_raw.faucet_id()
    {
        return Ok(None);
    }

    // 2. check that matcher won't lose assets matching
    {
        let a1: u128 = offer1_raw.amount().into();
        let b1: u128 = want1_raw.amount().into();
        let a2: u128 = want2_raw.amount().into();
        let b2: u128 = offer2_raw.amount().into();

        if a1
        .checked_mul(b2)
        .unwrap_or(0)  // on overflow, treat as “no match”
        < b1.checked_mul(a2).unwrap_or(u128::MAX)
        {
            return Ok(None);
        }
    }

    let offer_1_gt_want_2 = offer1_raw.amount() > want2_raw.amount();
    let offer_2_gt_want_1 = offer2_raw.amount() > want1_raw.amount();

    // ------------------------------------------------------------------------
    // Are both orders fully satisfiable?
    // – case A: each offer is *greater* than what the other side wants
    // – case B: each offer is *exactly equal* to what the other side wants
    //           *and* the asset IDs line up
    // ------------------------------------------------------------------------
    let both_fully_filled = (offer_1_gt_want_2 && offer_2_gt_want_1)
        || (offer1_raw.amount() == want2_raw.amount()
            && offer2_raw.amount() == want1_raw.amount()
            && (offer1_raw == want2_raw || offer2_raw == want1_raw));

    if both_fully_filled {
        // (optional) keep the debug line that was in the second branch only
        if !(offer_1_gt_want_2 && offer_2_gt_want_1) {
            println!("complete fill with arb");
        }

        // --------------------------------------------------------------------
        // Build the two P2ID notes – identical to what each branch did before
        // --------------------------------------------------------------------
        let note1_creator = creator_of(note1_in);
        let note2_creator = creator_of(note2_in);

        let note1_swap_cnt = note1_in.inputs().values()[8].as_int();
        let note2_swap_cnt = note2_in.inputs().values()[8].as_int();

        let note1_p2id_serial_num = get_p2id_serial_num(note1_in.serial_num(), note1_swap_cnt + 1);
        let note2_p2id_serial_num = get_p2id_serial_num(note2_in.serial_num(), note2_swap_cnt + 1);

        let p2id_from_1_to_2 = create_p2id_note(
            matcher,
            note1_creator,
            vec![want1_raw.into()], // exactly what note-1 wanted
            NoteType::Public,
            Felt::new(0),
            note1_p2id_serial_num,
        )
        .unwrap();

        let p2id_from_2_to_1 = create_p2id_note(
            matcher,
            note2_creator,
            vec![want2_raw.into()], // exactly what note-2 wanted
            NoteType::Public,
            Felt::new(0),
            note2_p2id_serial_num,
        )
        .unwrap();

        let note1_args = [
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
            Felt::new(want1_raw.amount()),
        ];
        let note2_args = [
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
            Felt::new(want2_raw.amount()),
        ];

        return Ok(Some(MatchedSwap {
            p2id_from_1_to_2,
            p2id_from_2_to_1,
            leftover_swapp_note: None,
            swap_note_1: note1_in.clone(),
            swap_note_2: note2_in.clone(),
            note1_args,
            note2_args,
        }));
    }

    // compute output amounts
    let (amount_a_1_p2id1, new_amount_a_note1, new_amount_b_note1) =
        compute_partial_swapp(offer1_raw.amount(), want1_raw.amount(), offer2_raw.amount());

    let (amount_a_1_p2id2, new_amount_a_note2, new_amount_b_note2) =
        compute_partial_swapp(offer2_raw.amount(), want2_raw.amount(), offer1_raw.amount());

    // ──one side must be fully filled ──────────────────────────
    let note1_filled = new_amount_a_note1 == 0 && new_amount_b_note1 == 0;
    let note2_filled = new_amount_a_note2 == 0 && new_amount_b_note2 == 0;

    let note1_swap_cnt = note1_in.inputs().values()[8].as_int();
    let note2_swap_cnt = note2_in.inputs().values()[8].as_int();

    println!("##############################################\n\n");

    println!("SWAP COUNT: {:?}", note1_swap_cnt);
    println!("SWAP COUNT: {:?}", note2_swap_cnt);

    println!("##############################################\n\n");

    println!("offer1_raw: {:?}", offer1_raw.amount());
    println!("want1_raw: {:?}", want1_raw.amount());
    println!("offer2_raw: {:?}", offer2_raw.amount());
    println!("want2_raw: {:?}", want2_raw.amount());

    println!("##############################################\n\n");
    println!("amount_a_1_p2id1: {:?}", amount_a_1_p2id1);
    println!("new_amount_a_note1: {:?}", new_amount_a_note1);
    println!("new_amount_b_note1: {:?}", new_amount_b_note1);
    println!("amount_a_1_p2id2: {:?}", amount_a_1_p2id2);
    println!("new_amount_a_note2: {:?}", new_amount_a_note2);
    println!("new_amount_b_note2: {:?}", new_amount_b_note2);

    // both notes still have leftover amounts → not a valid match
    if !note1_filled && !note2_filled {
        return Ok(None);
    }

    // Prices do not align, can't match orders
    if amount_a_1_p2id1 == 0 || amount_a_1_p2id2 == 0 {
        return Ok(None);
    }

    // make note 1 in, the note that is partially filled. ie. the note that has amounts left over
    // Make note1 the partially filled note
    let (
        note1_in,
        note2_in,
        _new_amount_a_note1,
        _new_amount_b_note1,
        _new_amount_a_note2,
        _new_amount_b_note2,
        _amount_a_1_p2id1,
        amount_a_1_p2id2,
    ) = if !note2_filled && note1_filled {
        // swap note1_in and note2_in (and all their associated data)
        println!("note 1 bigger");
        (
            note2_in,
            note1_in,
            new_amount_a_note2,
            new_amount_b_note2,
            new_amount_a_note1,
            new_amount_b_note1,
            amount_a_1_p2id2,
            amount_a_1_p2id1,
        )
    } else {
        println!("note 2 bigger");

        (
            note1_in,
            note2_in,
            new_amount_a_note1,
            new_amount_b_note1,
            new_amount_a_note2,
            new_amount_b_note2,
            amount_a_1_p2id1,
            amount_a_1_p2id2,
        )
    };

    let (offer1_raw, want1_raw) = decompose_swapp_note(note1_in)?;
    let (offer2_raw, want2_raw) = decompose_swapp_note(note2_in)?;

    println!("POST SWAP");
    println!("\n");
    println!("offer1_raw: {:?}", offer1_raw.amount());
    println!("want1_raw: {:?}", want1_raw.amount());
    println!("offer2_raw: {:?}", offer2_raw.amount());
    println!("want2_raw: {:?}", want2_raw.amount());
    println!("\n");

    let p2id_note1_output_requested_asset =
        FungibleAsset::new(want1_raw.faucet_id(), amount_a_1_p2id2).unwrap();

    let p2id_note2_output_requested_asset =
        FungibleAsset::new(want2_raw.faucet_id(), want2_raw.amount()).unwrap();

    let note1_creator = creator_of(note1_in);
    let note2_creator = creator_of(note2_in);

    let note1_swap_cnt = note1_in.inputs().values()[8].as_int();
    let note2_swap_cnt = note2_in.inputs().values()[8].as_int();
    let note1_p2id_serial_num = get_p2id_serial_num(note1_in.serial_num(), note1_swap_cnt + 1);
    let note2_p2id_serial_num = get_p2id_serial_num(note2_in.serial_num(), note2_swap_cnt + 1);

    let p2id_output_note1 = create_p2id_note(
        matcher,
        note1_creator,
        vec![p2id_note1_output_requested_asset.into()],
        NoteType::Public,
        Felt::new(0),
        note1_p2id_serial_num,
    )
    .unwrap();

    let p2id_output_note2 = create_p2id_note(
        matcher,
        note2_creator,
        vec![p2id_note2_output_requested_asset.into()],
        NoteType::Public,
        Felt::new(0),
        note2_p2id_serial_num,
    )
    .unwrap();

    let (offer1_raw, want1_raw) = decompose_swapp_note(note1_in)?;
    let (offer2_raw, want2_raw) = decompose_swapp_note(note2_in)?;

    let (_amount_a_1_p2id2, new_amount_a_note2, new_amount_b_note2) =
        compute_partial_swapp(offer1_raw.amount(), want1_raw.amount(), offer2_raw.amount());

    let note1_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(offer2_raw.amount()),
    ];

    let note2_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(want2_raw.amount()),
    ];

    // If complete order fill
    if new_amount_a_note1 == 0
        && new_amount_b_note1 == 0
        && new_amount_a_note2 == 0
        && new_amount_b_note2 == 0
    {
        println!("complete fill");
        return Ok(Some(MatchedSwap {
            p2id_from_1_to_2: p2id_output_note1,
            p2id_from_2_to_1: p2id_output_note2,
            leftover_swapp_note: None,
            swap_note_1: note1_in.clone(),
            swap_note_2: note2_in.clone(),
            note1_args,
            note2_args,
        }));
    }

    let mut sn = note1_in.serial_num();
    sn[3] = Felt::new(sn[3].as_int() + 1);
    let swap_cnt = note1_swap_cnt + 1;

    let leftover_swapp_note = Some(
        create_partial_swap_note(
            note1_creator,
            matcher,
            FungibleAsset::new(offer1_raw.faucet_id(), new_amount_a_note2)
                .unwrap()
                .into(),
            FungibleAsset::new(want1_raw.faucet_id(), new_amount_b_note2)
                .unwrap()
                .into(),
            sn,
            swap_cnt,
        )
        .unwrap(),
    );

    println!(
        "swap output: {:?}",
        leftover_swapp_note.clone().unwrap().id()
    );
    println!(
        "swap output: {:?}",
        leftover_swapp_note.clone().unwrap().serial_num()
    );
    println!(
        "swap output: {:?}",
        leftover_swapp_note.clone().unwrap().recipient().digest()
    );
    println!(
        "swap output asset id: {:?}",
        leftover_swapp_note.clone().unwrap().assets()
    );

    Ok(Some(MatchedSwap {
        p2id_from_1_to_2: p2id_output_note1,
        p2id_from_2_to_1: p2id_output_note2,
        leftover_swapp_note,
        swap_note_1: note1_in.clone(),
        swap_note_2: note2_in.clone(),
        note1_args,
        note2_args,
    }))
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

    create_partial_swap_note(
        creator,     // creator
        last_filler, // initially the same account
        offered.into(),
        requested.into(),
        serial,
        0, // not filled yet
    )
    .unwrap()
}

/// Generates and prints a comprehensive depth chart for USDC/ETH orderbook
pub fn generate_depth_chart(
    swap_notes: &[Note],
    faucet_usdc_id: &AccountId,
    _faucet_eth_id: &AccountId,
    account_names: &[(AccountId, &str)],
) {
    generate_depth_chart_with_options(
        swap_notes,
        faucet_usdc_id,
        _faucet_eth_id,
        account_names,
        true,
    );
}

/// Generates and prints a depth chart with optional detailed orderbook table
pub fn generate_depth_chart_with_options(
    swap_notes: &[Note],
    faucet_usdc_id: &AccountId,
    _faucet_eth_id: &AccountId,
    account_names: &[(AccountId, &str)],
    show_detailed_orderbook: bool,
) {
    let output = generate_depth_chart_string(
        swap_notes,
        faucet_usdc_id,
        _faucet_eth_id,
        account_names,
        show_detailed_orderbook,
    );
    print!("{}", output);
}

/// Generates a depth chart as a string with optional detailed orderbook table
pub fn generate_depth_chart_string(
    swap_notes: &[Note],
    faucet_usdc_id: &AccountId,
    _faucet_eth_id: &AccountId,
    account_names: &[(AccountId, &str)],
    show_detailed_orderbook: bool,
) -> String {
    let mut output = String::new();
    // ANSI color codes
    const GREEN: &str = "\x1b[32m";
    const RED: &str = "\x1b[31m";
    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";

    output.push_str("\n╔══════════════════════════════════════════════════════════╗\n");
    output.push_str(&format!(
        "║{}                 USDC/ETH ORDERBOOK DEPTH CHART           {}║\n",
        BOLD, RESET
    ));
    output.push_str("╚══════════════════════════════════════════════════════════╝\n");

    // Separate bids and asks
    let mut bids = Vec::new();
    let mut asks = Vec::new();

    for note in swap_notes {
        if let Ok((offered, requested)) = decompose_swapp_note(note) {
            let creator_id = creator_of(note);
            let creator_name = account_names
                .iter()
                .find(|(id, _)| *id == creator_id)
                .map(|(_, name)| *name)
                .unwrap_or("Unknown");

            // Check if this is a bid (buying ETH with USDC) or ask (selling ETH for USDC)
            if offered.faucet_id() == *faucet_usdc_id {
                // This is a bid (buy order)
                // For bids: price = USDC amount / ETH amount
                let usdc_amount = offered.amount() as f64;
                let eth_amount = requested.amount() as f64;

                // Ensure we don't divide by zero
                if eth_amount > 0.0 {
                    let price = usdc_amount / eth_amount;
                    bids.push((price, eth_amount, creator_name));
                }
            } else {
                // This is an ask (sell order)
                // For asks: price = USDC amount / ETH amount
                let usdc_amount = requested.amount() as f64;
                let eth_amount = offered.amount() as f64;

                // Ensure we don't divide by zero
                if eth_amount > 0.0 {
                    let price = usdc_amount / eth_amount;
                    asks.push((price, eth_amount, creator_name));
                }
            }
        }
    }

    // Sort bids by price (descending)
    bids.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

    // Sort asks by price (ascending)
    asks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Calculate cumulative volumes and prepare for visualization
    let mut bid_prices = Vec::new();
    let mut bid_volumes = Vec::new();
    let mut bid_cumulative = Vec::new();
    let mut bid_traders = Vec::new();

    let mut ask_prices = Vec::new();
    let mut ask_volumes = Vec::new();
    let mut ask_cumulative = Vec::new();
    let mut ask_traders = Vec::new();

    let mut cumulative_bid_volume_usd = 0.0;
    for (price, volume, trader) in &bids {
        bid_prices.push(*price);
        let volume_usd = volume * price;
        bid_volumes.push(volume_usd);
        cumulative_bid_volume_usd += volume_usd;
        bid_cumulative.push(cumulative_bid_volume_usd);
        bid_traders.push(trader);
    }

    let mut cumulative_ask_volume_usd = 0.0;
    for (price, volume, trader) in &asks {
        ask_prices.push(*price);
        let volume_usd = volume * price;
        ask_volumes.push(volume_usd);
        cumulative_ask_volume_usd += volume_usd;
        ask_cumulative.push(cumulative_ask_volume_usd);
        ask_traders.push(trader);
    }

    // Find the maximum cumulative volume for scaling
    let max_cumulative = f64::max(
        bid_cumulative.last().copied().unwrap_or(0.0),
        ask_cumulative.last().copied().unwrap_or(0.0),
    );

    // Calculate the spread
    let spread_info = if !bids.is_empty() && !asks.is_empty() {
        let best_bid = bids[0].0;
        let best_ask = asks[0].0;
        let spread = best_ask - best_bid;
        let spread_percentage = (spread / best_bid) * 100.0;
        let mid_price = (best_bid + best_ask) / 2.0;

        format!(
            "Best Bid: {:.2} USDC | Best Ask: {:.2} USDC | Spread: {:.2} ({:.2}%) | Mid: {:.2}",
            best_bid, best_ask, spread, spread_percentage, mid_price
        )
    } else {
        "No spread available - missing bids or asks".to_string()
    };

    // Print market summary
    output.push_str(
        "\n════════════════════════════════════════════════════════════════════════════════════════════╗\n"
    );
    output.push_str(
        "║                                MARKET SUMMARY                                             ║\n"
    );
    output.push_str(
        "╠═══════════════════════════════════════════════════════════════════════════════════════════╣\n"
    );
    output.push_str(&format!("║ {:<76}  ║\n", spread_info));
    output.push_str(
        "╚═══════════════════════════════════════════════════════════════════════════════════════════╝\n"
    );

    // Print the detailed orderbook table only if requested
    if show_detailed_orderbook {
        // Print the depth chart header
        output.push_str(
            "\n╔═══════════════════════════════════════════════════════════════════════════════════╗\n"
        );
        output.push_str(&format!(
            "║{}                               DEPTH CHART                                         {}║\n",
            BOLD, RESET
        ));
        output.push_str(
            "╠═══════════════════════════════════════════════════════════════════════════════════╣\n"
        );
        output.push_str(&format!(
            "║{}          BIDS (Buy Orders)            {}║{}          ASKS (Sell Orders)               {}║\n",
            GREEN, RESET, RED, RESET
        ));
        output.push_str(
            "╠═════════╦════════╦═════════╦══════════╬═════════╦════════╦═════════╦══════════════╣\n"
        );
        output.push_str(
            "║ Price   ║ ETH    ║ USD Vol ║ Trader   ║ Price   ║ ETH    ║ USD Vol ║ Trader       ║\n"
        );
        output.push_str(
            "╠═════════╬════════╬═════════╬══════════╬═════════╬════════╬═════════╬══════════════╣\n"
        );

        // Determine how many rows to display
        let max_rows = std::cmp::max(bids.len(), asks.len());

        // Print the depth chart rows
        for i in 0..max_rows {
            let bid_info = if i < bids.len() {
                format!(
                    "║{} {:<7.2} ║ {:<6.2} ║ {:<7.2} ║ {:<8} {}║",
                    GREEN, bid_prices[i], bid_volumes[i], bid_cumulative[i], bid_traders[i], RESET
                )
            } else {
                "║         ║        ║         ║          ║".to_string()
            };

            let ask_info = if i < asks.len() {
                format!(
                    " {:<7.2} ║ {:<6.2} ║ {:<7.2} ║ {:<12} ║",
                    ask_prices[i], ask_volumes[i], ask_cumulative[i], ask_traders[i]
                )
            } else {
                "         ║        ║         ║              ║".to_string()
            };

            output.push_str(&format!("{}{}\n", bid_info, ask_info));
        }

        output.push_str(
            "╚═════════╩════════╩═════════╩══════════╩═════════╩════════╩═════════╩══════════════╝\n"
        );
    }

    // Create a visual representation of the depth chart
    output.push_str("\n╔══════════════════════════════════════════════════════════╗\n");
    output.push_str("║                 VISUAL DEPTH CHART                       ║\n");
    output.push_str("╚══════════════════════════════════════════════════════════╝\n");

    // Define chart dimensions
    let chart_width = 60;
    let chart_height = 20;

    // Create price range
    let min_price = if !bid_prices.is_empty() && !ask_prices.is_empty() {
        f64::min(
            *bid_prices.last().unwrap_or(&0.0),
            *ask_prices.first().unwrap_or(&0.0),
        )
    } else if !bid_prices.is_empty() {
        *bid_prices.last().unwrap_or(&0.0)
    } else if !ask_prices.is_empty() {
        *ask_prices.first().unwrap_or(&0.0)
    } else {
        0.0
    };

    let max_price = if !bid_prices.is_empty() && !ask_prices.is_empty() {
        f64::max(
            *bid_prices.first().unwrap_or(&0.0),
            *ask_prices.last().unwrap_or(&0.0),
        )
    } else if !bid_prices.is_empty() {
        *bid_prices.first().unwrap_or(&0.0)
    } else if !ask_prices.is_empty() {
        *ask_prices.last().unwrap_or(&0.0)
    } else {
        0.0
    };

    // Add some padding to the price range
    let price_range = max_price - min_price;
    let min_price = min_price - (price_range * 0.05);
    let max_price = max_price + (price_range * 0.05);

    // Create a 2D grid for the chart
    let mut chart = vec![vec![' '; chart_width]; chart_height];

    // Draw the axes
    for y in 0..chart_height {
        chart[y][0] = '│';
    }
    for x in 0..chart_width {
        chart[chart_height - 1][x] = '─';
    }
    chart[chart_height - 1][0] = '└';

    // Draw the bid side (cumulative volume) - mark with 'B' for bids
    if !bid_cumulative.is_empty() {
        for i in 0..bid_prices.len() {
            let price_pos = ((bid_prices[i] - min_price) / (max_price - min_price)
                * (chart_width as f64 - 1.0)) as usize;
            let vol_pos = chart_height
                - 1
                - ((bid_cumulative[i] / max_cumulative) * (chart_height as f64 - 1.0)) as usize;
            if price_pos < chart_width && vol_pos < chart_height {
                chart[vol_pos][price_pos] = 'B'; // Mark as bid
            }
        }
    }

    // Draw the ask side (cumulative volume) - mark with 'A' for asks
    if !ask_cumulative.is_empty() {
        for i in 0..ask_prices.len() {
            let price_pos = ((ask_prices[i] - min_price) / (max_price - min_price)
                * (chart_width as f64 - 1.0)) as usize;
            let vol_pos = chart_height
                - 1
                - ((ask_cumulative[i] / max_cumulative) * (chart_height as f64 - 1.0)) as usize;
            if price_pos < chart_width && vol_pos < chart_height {
                chart[vol_pos][price_pos] = 'A'; // Mark as ask
            }
        }
    }

    // Print the chart
    output.push_str("  Volume\n");
    for y in 0..chart_height {
        output.push_str(&format!(
            "{:>8} ",
            if y == 0 {
                format!("{:.1}", max_cumulative)
            } else if y == chart_height - 1 {
                "0.0".to_string()
            } else if y == chart_height / 2 {
                format!("{:.1}", max_cumulative / 2.0)
            } else {
                "".to_string()
            }
        ));

        for x in 0..chart_width {
            match chart[y][x] {
                'B' => output.push_str(&format!("{}█{}", GREEN, RESET)), // Green for bids
                'A' => output.push_str(&format!("{}█{}", RED, RESET)),   // Red for asks
                c => output.push(c),
            }
        }
        output.push('\n');
    }

    // Print the price axis
    output.push_str("         ");
    let mut x = 0;
    while x < chart_width {
        if x == 0 || x == chart_width - 1 || x == chart_width / 2 {
            let price = min_price + (x as f64 / (chart_width - 1) as f64) * (max_price - min_price);
            output.push_str(&format!("{:.0}", price));
            x += 4; // Skip a few positions to avoid overlap
        } else if x % 10 == 0 {
            output.push('│');
            x += 1;
        } else {
            output.push(' ');
            x += 1;
        }
    }
    output.push_str("\n         Price (USDC per ETH)\n");

    // Print summary statistics
    output.push_str("\n╔══════════════════════════════════════════════════════════╗\n");
    output.push_str("║                  ORDERBOOK STATISTICS                    ║\n");
    output.push_str("╠══════════════════════════════════╦═══════════════════════╣\n");
    output.push_str(&format!(
        "║ Total number of orders           ║ {:<19}   ║\n",
        swap_notes.len()
    ));
    output.push_str(&format!(
        "║ Number of bid orders             ║ {:<19}   ║\n",
        bids.len()
    ));
    output.push_str(&format!(
        "║ Number of ask orders             ║ {:<19}   ║\n",
        asks.len()
    ));
    output.push_str(&format!(
        "║ Total bid volume (USD)           ║ ${:<18.4}   ║\n",
        bid_cumulative.last().unwrap_or(&0.0)
    ));
    output.push_str(&format!(
        "║ Total ask volume (USD)           ║ ${:<18.4}   ║\n",
        ask_cumulative.last().unwrap_or(&0.0)
    ));
    output.push_str("╚══════════════════════════════════╩═══════════════════════╝\n");

    output
}
