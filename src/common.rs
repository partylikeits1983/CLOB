use miden_assembly::{
    Assembler, DefaultSourceManager, LibraryPath,
    ast::{Module, ModuleKind},
};
use miden_crypto::dsa::rpo_falcon512::Polynomial;
use rand::{RngCore, rngs::StdRng};
use std::{
    error, fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::time::{Duration, sleep};

use miden_client::{
    Client, ClientError, Felt, Word,
    account::{
        Account, AccountBuilder, AccountId, AccountStorageMode, AccountType, StorageSlot,
        component::{BasicFungibleFaucet, BasicWallet, RpoFalcon512},
    },
    asset::{Asset, FungibleAsset, TokenSymbol},
    auth::AuthSecretKey,
    builder::ClientBuilder,
    crypto::{FeltRng, SecretKey},
    keystore::FilesystemKeyStore,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteExecutionMode, NoteId, NoteInputs, NoteMetadata,
        NoteRecipient, NoteRelevance, NoteScript, NoteTag, NoteType, build_swap_tag,
    },
    rpc::{Endpoint, TonicRpcClient},
    store::InputNoteRecord,
    transaction::{OutputNote, TransactionKernel, TransactionRequestBuilder, TransactionScript},
};
use miden_lib::note::utils;
use miden_objects::{
    Hasher, NoteError,
    account::{AccountComponent, StorageMap},
    assembly::Library,
};
use serde::de::value::Error;

// Signature verification code:
const N: usize = 512;
fn mul_modulo_p(a: Polynomial<Felt>, b: Polynomial<Felt>) -> [u64; 1024] {
    let mut c = [0; 2 * N];
    for i in 0..N {
        for j in 0..N {
            c[i + j] += a.coefficients[i].as_int() * b.coefficients[j].as_int();
        }
    }
    c
}

fn to_elements(poly: Polynomial<Felt>) -> Vec<Felt> {
    poly.coefficients.to_vec()
}

pub fn generate_advice_stack_from_signature(h: Polynomial<Felt>, s2: Polynomial<Felt>) -> Vec<u64> {
    let pi = mul_modulo_p(h.clone(), s2.clone());

    // lay the polynomials in order h then s2 then pi = h * s2
    let mut polynomials = to_elements(h.clone());
    polynomials.extend(to_elements(s2.clone()));
    polynomials.extend(pi.iter().map(|a| Felt::new(*a)));

    // get the challenge point and push it to the advice stack
    let digest_polynomials = Hasher::hash_elements(&polynomials);
    let challenge = (digest_polynomials[0], digest_polynomials[1]);
    let mut advice_stack = vec![challenge.0.as_int(), challenge.1.as_int()];

    // push the polynomials to the advice stack
    let polynomials: Vec<u64> = polynomials.iter().map(|&e| e.into()).collect();
    advice_stack.extend_from_slice(&polynomials);

    advice_stack
}

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
    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key().clone()))
        .with_component(BasicWallet);
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair.clone()))
        .unwrap();

    Ok((account, key_pair))
}

pub async fn create_basic_faucet(
    client: &mut Client,
    keystore: FilesystemKeyStore<StdRng>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = SecretKey::with_rng(client.rng());
    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let symbol = TokenSymbol::new("MID").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000_000);
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_component(RpoFalcon512::new(key_pair.public_key()))
        .with_component(BasicFungibleFaucet::new(symbol, decimals, max_supply).unwrap());
    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;
    keystore
        .add_key(&AuthSecretKey::RpoFalcon512(key_pair))
        .unwrap();
    Ok(account)
}

pub async fn create_signature_check_account(
    client: &mut Client,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let file_path = Path::new("./masm/accounts/account_signature_check.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    let empty_storage_slot = StorageSlot::empty_value();
    let storage_map = StorageMap::new();
    let storage_slot_map = StorageSlot::Map(storage_map.clone());

    let account_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![empty_storage_slot, storage_slot_map],
    )
    .unwrap()
    .with_supports_all_types();

    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(BasicWallet)
        .with_component(account_component);

    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;

    Ok(account)
}

pub async fn create_multisig_poc(
    client: &mut Client,
    authed_pub_keys: Vec<Word>,
) -> Result<miden_client::account::Account, ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let file_path = Path::new("./masm/accounts/signature_check_loop.masm");
    let account_code = fs::read_to_string(file_path).unwrap();

    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);

    let empty_storage_slot = StorageSlot::empty_value();
    // Declare storage_map as mutable
    let mut storage_map = StorageMap::new();

    let true_value = [Felt::new(1), Felt::new(1), Felt::new(1), Felt::new(1)];

    // Iterate over each key in the vector
    for key in authed_pub_keys.iter() {
        storage_map.insert(key.into(), true_value);
    }

    let storage_slot_map = StorageSlot::Map(storage_map.clone());

    let account_component = AccountComponent::compile(
        account_code.clone(),
        assembler.clone(),
        vec![empty_storage_slot, storage_slot_map],
    )
    .unwrap()
    .with_supports_all_types();

    let anchor_block = client.get_latest_epoch_block().await.unwrap();
    // let anchor_block = client.get_epoch_block(10.into()).await.unwrap();
    let builder = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        // .with_component(BasicWallet)
        .with_component(account_component);

    let (account, seed) = builder.build().unwrap();
    client.add_account(&account, Some(seed), false).await?;

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
            let tx_request = TransactionRequestBuilder::mint_fungible_asset(
                asset,
                account.id(),
                NoteType::Public,
                client.rng(),
            )
            .unwrap()
            .build()
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
                .with_authenticated_input_notes([(note.id(), None)])
                .build()
                .unwrap();

            let tx_exec = client.new_transaction(account.id(), consume_req).await?;
            client.submit_transaction(tx_exec).await?;
        }
    }
    client.sync_state().await?;

    Ok((accounts, faucets))
}

pub async fn mint_from_faucet_for_matcher(
    client: &mut Client,
    account: &Account,
    faucet: &Account,
    amount: u64,
) -> Result<(), ClientError> {
    if amount == 0 {
        return Ok(());
    }

    let asset = FungibleAsset::new(faucet.id(), amount).unwrap();
    let mint_req = TransactionRequestBuilder::mint_fungible_asset(
        asset,
        account.id(),
        NoteType::Public,
        client.rng(),
    )?
    .build()?;
    let mint_exec = client.new_transaction(faucet.id(), mint_req).await?;
    client.submit_transaction(mint_exec.clone()).await?;

    let minted_note = match mint_exec.created_notes().get_note(0) {
        OutputNote::Full(note) => note.clone(),
        _ => panic!("Expected full minted note"),
    };

    wait_for_notes(client, account, 1).await?;
    client.sync_state().await?;

    // let script_code = fs::read_to_string(Path::new("./masm/scripts/match_script.masm")).unwrap();
    // let matcher_code =
    //    fs::read_to_string(Path::new("./masm/accounts/two_to_one_match.masm")).unwrap();
    // let matcher_library =
    //    create_library_simplified(matcher_code, "external_contract::matcher_contract").unwrap();

    // let tx_script = create_tx_script(script_code, Some(matcher_library)).unwrap();

    let consume_req = TransactionRequestBuilder::new()
        .with_authenticated_input_notes([(minted_note.id(), None)])
        // .with_custom_script(tx_script)
        .build()?;

    let consume_exec = client.new_transaction(account.id(), consume_req).await?;
    client.submit_transaction(consume_exec).await?;
    client.sync_state().await?;

    Ok(())
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
    let p2id_tag = NoteTag::from_account_id(creator, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.inner().into(),
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
    let p2id_tag = NoteTag::from_account_id(creator, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        swapp_tag.inner().into(),
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
        .with_own_output_notes(vec![OutputNote::Full(swapp_note.clone())])
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
    let tag = NoteTag::from_account_id(target, NoteExecutionMode::Local)?;

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
    let payback_tag = NoteTag::from_account_id(underwriter, NoteExecutionMode::Local)?;

    let inputs = NoteInputs::new(vec![
        payback_recipient_word[0],
        payback_recipient_word[1],
        payback_recipient_word[2],
        payback_recipient_word[3],
        requested_asset_word[0],
        requested_asset_word[1],
        requested_asset_word[2],
        requested_asset_word[3],
        payback_tag.inner().into(),
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
    let amount_out_offered = offered_swapp_asset_amount
        .saturating_mul(requested_asset_filled)
        .saturating_div(requested_swapp_asset_amount);

    // update leftover offered amount
    let new_offered_asset_amount = offered_swapp_asset_amount.saturating_sub(amount_out_offered);

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
        .with_rpc(rpc_api.clone())
        .with_filesystem_keystore("./keystore")
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
    let endpoint = Endpoint::localhost();
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(TonicRpcClient::new(&endpoint, timeout_ms));
    let mut client = ClientBuilder::new()
        .with_rpc(rpc_api.clone())
        .with_filesystem_keystore("./keystore")
        .in_debug_mode(true)
        .build()
        .await?;
    let anchor_block = client.get_latest_epoch_block().await.unwrap();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let (counter_contract, counter_seed) = AccountBuilder::new(init_seed)
        .anchor((&anchor_block).try_into().unwrap())
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(counter_component.clone())
        .with_component(BasicWallet)
        .build()
        .unwrap();

    Ok((counter_contract, counter_seed))
}

// Creates public note
pub async fn create_public_note(
    client: &mut Client,
    note_code: String,
    account_library: Library,
    creator_account: Account,
    assets: NoteAssets,
) -> Result<Note, Error> {
    let assembler = TransactionKernel::assembler()
        .with_library(&account_library)
        .unwrap()
        .with_debug_mode(true);
    let rng = client.rng();
    let serial_num = rng.draw_word();
    let note_script = NoteScript::compile(note_code, assembler.clone()).unwrap();
    let note_inputs = NoteInputs::new([].to_vec()).unwrap();
    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs.clone());
    let tag = NoteTag::for_public_use_case(0, 0, NoteExecutionMode::Local).unwrap();
    let metadata = NoteMetadata::new(
        creator_account.id(),
        NoteType::Public,
        tag,
        NoteExecutionHint::always(),
        Felt::new(0),
    )
    .unwrap();

    let note = Note::new(assets, metadata, recipient);

    let note_req = TransactionRequestBuilder::new()
        .with_own_output_notes(vec![OutputNote::Full(note.clone())])
        .build()
        .unwrap();
    let tx_result = client
        .new_transaction(creator_account.id(), note_req)
        .await
        .unwrap();

    let _ = client.submit_transaction(tx_result).await;
    client.sync_state().await.unwrap();

    Ok(note)
}

// Waits for note
pub async fn wait_for_note(
    client: &mut Client,
    account_id: &Account,
    expected: &Note,
) -> Result<(), ClientError> {
    loop {
        client.sync_state().await?;

        let notes: Vec<(InputNoteRecord, Vec<(AccountId, NoteRelevance)>)> =
            client.get_consumable_notes(Some(account_id.id())).await?;

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

pub fn create_tx_script(
    script_code: String,
    library: Option<Library>,
) -> Result<TransactionScript, Error> {
    let assembler = TransactionKernel::assembler();

    let assembler = match library {
        Some(lib) => assembler.with_library(lib),
        None => Ok(assembler.with_debug_mode(true)),
    }
    .unwrap();
    let tx_script = TransactionScript::compile(script_code, [], assembler).unwrap();

    Ok(tx_script)
}

// Creates library
pub fn create_library_simplified(
    account_code: String,
    library_path: &str,
) -> Result<miden_assembly::Library, Box<dyn std::error::Error>> {
    let assembler: Assembler = TransactionKernel::assembler().with_debug_mode(true);
    let source_manager = Arc::new(DefaultSourceManager::default());
    let module = Module::parser(ModuleKind::Library).parse_str(
        LibraryPath::new(library_path)?,
        account_code,
        &source_manager,
    )?;
    let library = assembler.clone().assemble_library([module])?;
    Ok(library)
}

pub fn create_exact_p2id_note(
    sender: AccountId,
    target: AccountId,
    assets: Vec<Asset>,
    note_type: NoteType,
    aux: Felt,
    serial_num: Word,
) -> Result<Note, NoteError> {
    let recipient = utils::build_p2id_recipient(target, serial_num)?;

    let tag = NoteTag::from_account_id(target, NoteExecutionMode::Local)?;

    let metadata = NoteMetadata::new(sender, note_type, tag, NoteExecutionHint::always(), aux)?;
    let vault = NoteAssets::new(assets)?;

    Ok(Note::new(vault, metadata, recipient))
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

    /// `note_args` that **must** be supplied when the matcher consumes
    /// *maker*’s SWAPP note (`note1`).
    pub note1_args: [Felt; 4],

    /// `note_args` that **must** be supplied when the matcher consumes
    /// *taker*’s SWAPP note (`note2`).
    pub note2_args: [Felt; 4],
}

impl fmt::Debug for MatchedSwap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // --------------------------------------------------------------------
        // helper: pretty-print all fungible assets inside a note
        // --------------------------------------------------------------------
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

        // --------------------------------------------------------------------
        // helper: pretty-print offered / requested of a SWAPP note
        // --------------------------------------------------------------------
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

        // gather the three fields
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
    //-----------------------------------------------------------------------
    // 0. Decode both SWAPP notes
    //-----------------------------------------------------------------------
    let (offer1_raw, want1_raw) = decompose_swapp_note(note1_in)?;
    let (offer2_raw, want2_raw) = decompose_swapp_note(note2_in)?;

    // They must be the inverse pair (A,B) ↔ (B,A)
    if offer1_raw.faucet_id() != want2_raw.faucet_id()
        || want1_raw.faucet_id() != offer2_raw.faucet_id()
    {
        return Ok(None);
    }

    //-----------------------------------------------------------------------
    // 1. Make sure `note1` is the *larger* order (cannot be completely filled
    //    by the other).  A simple heuristic works well in practice:
    //        if maker.stock < taker.demand  -> swap.
    //-----------------------------------------------------------------------
    let (note1, note2, offer1, want1, offer2, want2) = if offer1_raw.amount() < want2_raw.amount() {
        // swap so that `note1` always has “more liquidity”
        (
            note2_in, note1_in, offer2_raw, want2_raw, offer1_raw, want1_raw,
        )
    } else {
        (
            note1_in, note2_in, offer1_raw, want1_raw, offer2_raw, want2_raw,
        )
    };

    //-----------------------------------------------------------------------
    // 2. Limit-price check (taker_price ≥ maker_price?)
    //-----------------------------------------------------------------------
    let maker_num = want1.amount(); // quote
    let maker_den = offer1.amount(); // base
    let taker_num = offer2.amount(); // quote
    let taker_den = want2.amount(); // base

    if (taker_num as u128) * (maker_den as u128) < (maker_num as u128) * (taker_den as u128) {
        return Ok(None); // prices do not cross
    }

    //-----------------------------------------------------------------------
    // 3. Decide **only** how many quote tokens the taker will pay.
    //    Everything else comes from `compute_partial_swapp`.
    //-----------------------------------------------------------------------
    let max_by_supply = offer2.amount(); // what taker *has*
    let max_by_maker = want1.amount(); // what maker *asks*
    let max_by_demand =
        (want2.amount() as u128 * want1.amount() as u128 / offer1.amount() as u128) as u64; // taker must not
    // receive > want2
    let fill_quote = max_by_supply.min(max_by_maker).min(max_by_demand);
    if fill_quote == 0 {
        return Ok(None);
    }

    // Single source of truth for the rest
    let (fill_base, leftover_base, leftover_quote) =
        compute_partial_swapp(offer1.amount(), want1.amount(), fill_quote);

    debug_assert!(fill_base > 0);
    debug_assert!(leftover_base + fill_base == offer1.amount());
    debug_assert!(leftover_quote + fill_quote == want1.amount());

    //-----------------------------------------------------------------------
    // 4. Build the two P2ID notes (matcher ↔ users)
    //-----------------------------------------------------------------------
    let maker_id = creator_of(note1);
    let taker_id = creator_of(note2);

    let asset_base = FungibleAsset::new(offer1.faucet_id(), fill_base)
        .unwrap()
        .into();
    let asset_quote = FungibleAsset::new(want1.faucet_id(), fill_quote)
        .unwrap()
        .into();

    // serial-numbers (swap-count sits in limb-8 of the inputs)
    let note1_swap_cnt = note1.inputs().values()[8].as_int();
    let note2_swap_cnt = note2.inputs().values()[8].as_int();

    let p2id_1_sn = get_p2id_serial_num(note1.serial_num(), note1_swap_cnt + 1);
    let p2id_2_sn = get_p2id_serial_num(note2.serial_num(), note2_swap_cnt + 1);

    let p2id_from_1_to_2 = create_p2id_note(
        matcher,
        maker_id,
        vec![asset_quote],
        NoteType::Public,
        Felt::new(0),
        p2id_1_sn,
    )
    .unwrap();

    let p2id_from_2_to_1 = create_p2id_note(
        matcher,
        taker_id,
        vec![asset_base],
        NoteType::Public,
        Felt::new(0),
        p2id_2_sn,
    )
    .unwrap();

    //-----------------------------------------------------------------------
    // 5. Note-arguments (match your new semantics)
    //-----------------------------------------------------------------------
    let note1_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_quote),
    ];
    let note2_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(fill_base),
    ];

    //-----------------------------------------------------------------------
    // 6. Possible leftover part of the *maker* (note 1)
    //-----------------------------------------------------------------------
    let leftover_swapp_note = if leftover_base > 0 {
        // bump {serial_num, swap_cnt} exactly like the partial-consume circuit
        let mut sn = note1.serial_num();
        sn[3] = Felt::new(sn[3].as_int() + 1);
        let swap_cnt = note1_swap_cnt + 1;

        Some(
            create_partial_swap_note(
                maker_id,
                matcher, // last counter-party is the matcher
                FungibleAsset::new(offer1.faucet_id(), leftover_base)
                    .unwrap()
                    .into(),
                FungibleAsset::new(want1.faucet_id(), leftover_quote)
                    .unwrap()
                    .into(),
                sn,
                swap_cnt,
            )
            .unwrap(),
        )
    } else {
        None
    };

    //-----------------------------------------------------------------------
    // 7. All done
    //-----------------------------------------------------------------------
    Ok(Some(MatchedSwap {
        p2id_from_1_to_2,
        p2id_from_2_to_1,
        leftover_swapp_note,
        note1_args,
        note2_args,
    }))
}
