use serde::de::value::Error;
use std::{env, fmt, fs, path::PathBuf};

use miden_client::{
    account::AccountId,
    asset::{Asset, FungibleAsset},
    note::{
        build_swap_tag, Note, NoteAssets, NoteExecutionHint, NoteInputs, NoteMetadata,
        NoteRecipient, NoteTag, NoteType,
    },
    Felt, Word, ScriptBuilder,
};

use miden_objects::{Hasher, NoteError};

pub fn create_partial_swap_note(
    creator: AccountId,
    last_consumer: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    swap_serial_num: [Felt; 4],
    swap_count: u64,
) -> Result<Note, NoteError> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "..", "masm", "notes", "SWAPP.masm"]
        .iter()
        .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let note_script = ScriptBuilder::new(true).compile_note_script(note_code).unwrap();
    let note_type = NoteType::Public;

    let requested_asset_word: Word = requested_asset.into();
    let swapp_tag = build_swap_tag(note_type, &offered_asset, &requested_asset)?;
    let p2id_tag = NoteTag::from_account_id(creator);

    println!("HERE: {:?}", requested_asset_word);

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
    let recipient = NoteRecipient::new(swap_serial_num.into(), note_script.clone(), inputs.clone());
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
    let path: PathBuf = [
        manifest_dir,
        "..",
        "masm",
        "notes",
        "SWAPP_cancellable.masm",
    ]
    .iter()
    .collect();

    let note_code = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("Error reading {}: {}", path.display(), err));

    let note_script = ScriptBuilder::new(true).compile_note_script(note_code).unwrap();
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
    let recipient = NoteRecipient::new(swap_serial_num.into(), note_script.clone(), inputs.clone());
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

    // Handle AccountId creation more gracefully with detailed logging
    let requested_id = match AccountId::try_from([requested[3], requested[2]]) {
        Ok(id) => id,
        Err(e) => {
            eprintln!(
                "❌ Error creating AccountId from [{}, {}]: {}",
                requested[3], requested[2], e
            );
            eprintln!(
                "📝 Full note inputs ({} total): {:?}",
                note_inputs.len(),
                note_inputs
            );
            eprintln!(
                "🔍 First 4 inputs (requested asset): {:?}",
                &note_inputs[..4]
            );
            if note_inputs.len() >= 14 {
                eprintln!(
                    "🔍 Creator inputs [12-13]: [{}, {}]",
                    note_inputs[12], note_inputs[13]
                );
            }
            // Return a more specific error
            panic!("Failed to create AccountId from note inputs: {}", e);
        }
    };

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

    /// Remaining piece of the maker's order, if it was not filled
    /// completely. `None` means the maker was filled in full.
    pub leftover_swapp_note: Option<Note>,

    // Input Note 1
    pub swap_note_1: Note,

    // Input Note 2
    pub swap_note_2: Note,

    /// `note_args` that **must** be supplied when the matcher consumes
    /// *maker*'s SWAPP note (`note1`).
    pub note1_args: [Felt; 4],

    /// `note_args` that **must** be supplied when the matcher consumes
    /// *taker*'s SWAPP note (`note2`).
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
        .unwrap_or(0)  // on overflow, treat as "no match"
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

        let note1_p2id_serial_num = get_p2id_serial_num(*note1_in.serial_num(), note1_swap_cnt + 1);
        let note2_p2id_serial_num = get_p2id_serial_num(*note2_in.serial_num(), note2_swap_cnt + 1);

        let p2id_from_1_to_2 = crate::notes::create_p2id_note(
            matcher,
            note1_creator,
            vec![want1_raw.into()], // exactly what note-1 wanted
            NoteType::Public,
            Felt::new(0),
            note1_p2id_serial_num,
        )
        .unwrap();

        let p2id_from_2_to_1 = crate::notes::create_p2id_note(
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

    // Determine which note is the maker (partially filled) and which is the taker (fully filled)
    // The maker is the one with the larger order that will be partially filled
    let (maker_note, taker_note, swapped) = {
        // Calculate the fill ratios to determine which order is larger
        let ratio1 = (offer1_raw.amount() as f64) / (want2_raw.amount() as f64);
        let ratio2 = (offer2_raw.amount() as f64) / (want1_raw.amount() as f64);

        // The order with the higher ratio is the maker (will be partially filled)
        if ratio1 > ratio2 {
            (note1_in, note2_in, false)
        } else {
            (note2_in, note1_in, true)
        }
    };

    // Decompose the reordered notes
    let (maker_offer, maker_want) = decompose_swapp_note(maker_note)?;
    let (taker_offer, taker_want) = decompose_swapp_note(taker_note)?;

    // Compute the partial swap for the maker note
    let (amount_out_maker, new_maker_offer, new_maker_want) = compute_partial_swapp(
        maker_offer.amount(),
        maker_want.amount(),
        taker_offer.amount(),
    );

    // The taker gets exactly what they want
    let amount_out_taker = taker_want.amount();

    println!("##############################################\n\n");
    println!(
        "SWAP COUNT maker: {:?}",
        maker_note.inputs().values()[8].as_int()
    );
    println!(
        "SWAP COUNT taker: {:?}",
        taker_note.inputs().values()[8].as_int()
    );
    println!("##############################################\n\n");
    println!("maker_offer: {:?}", maker_offer.amount());
    println!("maker_want: {:?}", maker_want.amount());
    println!("taker_offer: {:?}", taker_offer.amount());
    println!("taker_want: {:?}", taker_want.amount());
    println!("##############################################\n\n");
    println!("amount_out_maker: {:?}", amount_out_maker);
    println!("new_maker_offer: {:?}", new_maker_offer);
    println!("new_maker_want: {:?}", new_maker_want);
    println!("amount_out_taker: {:?}", amount_out_taker);

    // Verify the match is valid
    if amount_out_maker == 0 || amount_out_taker == 0 {
        return Ok(None);
    }

    // Get creator IDs and swap counts
    let maker_creator = creator_of(maker_note);
    let taker_creator = creator_of(taker_note);

    let maker_swap_cnt = maker_note.inputs().values()[8].as_int();
    let taker_swap_cnt = taker_note.inputs().values()[8].as_int();

    let maker_p2id_serial_num = get_p2id_serial_num(*maker_note.serial_num(), maker_swap_cnt + 1);
    let taker_p2id_serial_num = get_p2id_serial_num(*taker_note.serial_num(), taker_swap_cnt + 1);

    // Create P2ID notes for the matched amounts
    let p2id_to_maker = crate::notes::create_p2id_note(
        matcher,
        maker_creator,
        vec![FungibleAsset::new(maker_want.faucet_id(), amount_out_maker)
            .unwrap()
            .into()],
        NoteType::Public,
        Felt::new(0),
        maker_p2id_serial_num,
    )
    .unwrap();

    let p2id_to_taker = crate::notes::create_p2id_note(
        matcher,
        taker_creator,
        vec![FungibleAsset::new(taker_want.faucet_id(), amount_out_taker)
            .unwrap()
            .into()],
        NoteType::Public,
        Felt::new(0),
        taker_p2id_serial_num,
    )
    .unwrap();

    // Set up note arguments
    let maker_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(taker_offer.amount()),
    ];

    let taker_args = [
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
        Felt::new(amount_out_taker),
    ];

    // Check if this is a complete fill
    let is_complete_fill = new_maker_offer == 0 && new_maker_want == 0;

    let leftover_swapp_note = if !is_complete_fill {
        // Create the leftover SWAPP note for the maker
        let mut sn = maker_note.serial_num();
        sn[3] = Felt::new(sn[3].as_int() + 1);
        let swap_cnt = maker_swap_cnt + 1;

        Some(
            create_partial_swap_note(
                maker_creator,
                matcher,
                FungibleAsset::new(maker_offer.faucet_id(), new_maker_offer)
                    .unwrap()
                    .into(),
                FungibleAsset::new(maker_want.faucet_id(), new_maker_want)
                    .unwrap()
                    .into(),
                *sn,
                swap_cnt,
            )
            .unwrap(),
        )
    } else {
        println!("complete fill");
        None
    };

    if let Some(ref leftover) = leftover_swapp_note {
        println!("swap output: {:?}", leftover.id());
        println!("swap output: {:?}", leftover.serial_num());
        println!("swap output: {:?}", leftover.recipient().digest());
        println!("swap output asset id: {:?}", leftover.assets());
    }

    // Return the result with notes in the original order
    let (final_p2id_1, final_p2id_2, final_note1, final_note2, final_args1, final_args2) =
        if !swapped {
            (
                p2id_to_maker,
                p2id_to_taker,
                maker_note.clone(),
                taker_note.clone(),
                maker_args,
                taker_args,
            )
        } else {
            (
                p2id_to_taker,
                p2id_to_maker,
                taker_note.clone(),
                maker_note.clone(),
                taker_args,
                maker_args,
            )
        };

    Ok(Some(MatchedSwap {
        p2id_from_1_to_2: final_p2id_1,
        p2id_from_2_to_1: final_p2id_2,
        leftover_swapp_note,
        swap_note_1: final_note1,
        swap_note_2: final_note2,
        note1_args: final_args1,
        note2_args: final_args2,
    }))
}
