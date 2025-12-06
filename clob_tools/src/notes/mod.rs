use std::{env, fs, path::PathBuf};

use miden_client::{
    account::AccountId,
    asset::Asset,
    crypto::FeltRng,
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteInputs, NoteMetadata, NoteRecipient, NoteScript,
        NoteTag, NoteType,
    },
    transaction::TransactionKernel,
    Felt,
};

use miden_objects::NoteError;

pub fn create_p2id_note(
    sender: AccountId,
    target: AccountId,
    assets: Vec<Asset>,
    note_type: NoteType,
    aux: Felt,
    serial_num: [Felt; 4],
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "..", "masm", "notes", "P2ID.masm"]
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
    let path: PathBuf = [
        manifest_dir,
        "..",
        "masm",
        "notes",
        "option_contract_note.masm",
    ]
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

    let payback_recipient_word: miden_client::Word = p2id_note.recipient().digest().into();
    let requested_asset_word: miden_client::Word = requested_asset.into();
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
    let tag = miden_client::note::build_swap_tag(note_type, &offered_asset, &requested_asset)?;
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
