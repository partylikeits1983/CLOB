use miden_client::{Word, account::AccountId, asset::FungibleAsset};
use miden_clob::{
    common::{
        create_partial_swap_note, decompose_swapp_note, price_to_swap_note, try_match_swapp_notes,
    },
    compute_partial_swapp,
};

#[test]
fn test_try_match_swapp_notes_arithmetic() {
    // ────────────────────────────────────────────────────────────
    // 1.  Fixture: two opposite SWAPP orders
    // ────────────────────────────────────────────────────────────

    let maker_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let taker_id = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();
    let matcher = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();

    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    // 100 A for 100 B
    let maker_note = create_partial_swap_note(
        maker_id,
        maker_id,                                          // last counter-party (self)
        FungibleAsset::new(faucet_a, 100).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 100).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // taker: 50 B  → 50 A
    let taker_note = create_partial_swap_note(
        taker_id,
        taker_id,
        FungibleAsset::new(faucet_b, 50).unwrap().into(), // offered
        FungibleAsset::new(faucet_a, 50).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 2.  Run the matcher
    // ────────────────────────────────────────────────────────────
    let swap = try_match_swapp_notes(&maker_note, &taker_note, matcher)
        .unwrap()
        .expect("orders should cross");

    // ────────────────────────────────────────────────────────────
    // 3.  Assertions
    // ────────────────────────────────────────────────────────────
    // 3-a. P2ID amounts
    let p2id_a_out = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    assert_eq!(p2id_a_out.amount(), 50);
    assert_eq!(p2id_a_out.faucet_id(), faucet_b);

    let p2id_b_out = swap
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_b_out.amount(), 50);
    assert_eq!(p2id_b_out.faucet_id(), faucet_a);

    // 3-b. Left-over maker SWAPP note (should be 25 A → 25 B)
    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("maker not 100 % filled");
    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();
    assert_eq!(left_off.amount(), 50);
    assert_eq!(left_off.faucet_id(), faucet_a);
    assert_eq!(left_req.amount(), 50);
    assert_eq!(left_req.faucet_id(), faucet_b);

    // 3-c. Note-args semantics
    assert_eq!(swap.note1_args[3].as_int(), 50); // maker receives 25 B
    assert_eq!(swap.note2_args[3].as_int(), 50); // taker receives 25 A
}

#[test]
fn test_try_match_swapp_notes_arithmetic_case2() {
    // ────────────────────────────────────────────────────────────
    // 1.  Fixture
    //      maker : 150 A  →  90 B   (price = 0.6 B per A)
    //      taker :  60 B  →  50 A   (price = 1.2 B per A)
    //
    //      ⇒ prices cross (1.2 ≥ 0.6)
    //      ⇒ taker can pay at most 60 B, maker asks 90 B,
    //         taker must not exceed (want₂·want₁ / offer₁) = (50·90)/150 = 30 B
    //      ⇒ fill_quote  = 30 B
    //         fill_base   = ⌊150·30 / 90⌋ = 50 A
    //         leftovers   = 100 A  /  60 B
    // ────────────────────────────────────────────────────────────
    let maker_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let taker_id = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();
    let matcher = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    // maker: 150 A  → 90 B
    let maker_note = create_partial_swap_note(
        maker_id,
        maker_id,
        FungibleAsset::new(faucet_a, 150).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 90).unwrap().into(),  // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // taker: 60 B  → 50 A
    let taker_note = create_partial_swap_note(
        taker_id,
        taker_id,
        FungibleAsset::new(faucet_b, 60).unwrap().into(), // offered
        FungibleAsset::new(faucet_a, 50).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 2. Run the matcher
    // ────────────────────────────────────────────────────────────
    let swap = try_match_swapp_notes(&maker_note, &taker_note, matcher)
        .unwrap()
        .expect("orders should cross");

    // ────────────────────────────────────────────────────────────
    // 3. Assertions
    // ────────────────────────────────────────────────────────────
    // 3-a.  P2ID payloads
    let p2id_a_out = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_a_out.amount(), 30); // 30 B to taker
    assert_eq!(p2id_a_out.faucet_id(), faucet_b);

    let p2id_b_out = swap
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_b_out.amount(), 50); // 50 A to maker
    assert_eq!(p2id_b_out.faucet_id(), faucet_a);

    // 3-b.  Left-over maker order: 100 A → 60 B
    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("maker not 100 % filled");
    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();
    assert_eq!(left_off.amount(), 100);
    assert_eq!(left_off.faucet_id(), faucet_a);
    assert_eq!(left_req.amount(), 60);
    assert_eq!(left_req.faucet_id(), faucet_b);

    // 3-c.  Note-arg limb-3 semantics
    assert_eq!(swap.note1_args[3].as_int(), 30); // maker receives 30 B
    assert_eq!(swap.note2_args[3].as_int(), 50); // taker receives 50 A
}

#[test]
fn test_try_match_swapp_notes_arithmetic_case3() {
    let maker_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let taker_id = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();
    let matcher = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    let maker_note = create_partial_swap_note(
        maker_id,
        maker_id,
        FungibleAsset::new(faucet_a, 1815515).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 689).unwrap().into(),     // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // taker: 60 B  → 50 A
    let taker_note = create_partial_swap_note(
        taker_id,
        taker_id,
        FungibleAsset::new(faucet_b, 352).unwrap().into(), // offered
        FungibleAsset::new(faucet_a, 912736).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 2. Run the matcher
    // ────────────────────────────────────────────────────────────
    let swap = try_match_swapp_notes(&maker_note, &taker_note, matcher)
        .unwrap()
        .expect("orders should cross");

    // ────────────────────────────────────────────────────────────
    // 3. Assertions
    // ────────────────────────────────────────────────────────────
    // 3-a.  P2ID payloads
    let p2id_a_out = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_a_out.amount(), 1498024); // 30 B to taker
    assert_eq!(p2id_a_out.faucet_id(), faucet_b);

    let p2id_b_out = swap
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_b_out.amount(), 587); // 50 A to maker
    assert_eq!(p2id_b_out.faucet_id(), faucet_a);

    // 3-b.  Left-over maker order: 100 A → 60 B
    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("maker not 100 % filled");
    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();
    assert_eq!(left_off.amount(), 3);
    assert_eq!(left_off.faucet_id(), faucet_a);
    assert_eq!(left_req.amount(), 5662);
    assert_eq!(left_req.faucet_id(), faucet_b);

    // 3-c.  Note-arg limb-3 semantics
    assert_eq!(swap.note1_args[3].as_int(), 587); // maker receives 30 B
    assert_eq!(swap.note2_args[3].as_int(), 1498024); // taker receives 50 A
}

#[tokio::test]
async fn test_compute_partial_swapp_math() {
    let amount_b_in = 25;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        100,         // originally offered A
        50,          // originally requested B
        amount_b_in, // Bob fills 25 B
    );

    println!("==== test_compute_partial_swapp ====");
    println!("amount_a_1 (A out):          {}", amount_a_1);
    println!("amount_b_1 (B in):           {}", amount_b_in);
    println!("new_amount_a (A leftover):   {}", new_amount_a);
    println!("new_amount_b (B leftover):   {}", new_amount_b);

    assert_eq!(amount_a_1, 50);
    assert_eq!(amount_b_in, 25);
    assert_eq!(new_amount_a, 50);
    assert_eq!(new_amount_b, 25);

    let amount_b_in = 2500;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        5000,        // originally offered A
        5000,        // originally requested B
        amount_b_in, // Bob fills 2500 B
    );

    // 1. For a 1:1 ratio: 2500 B in => 2500 A out => leftover 2500 A / 2500 B
    assert_eq!(amount_a_1, 2500);
    assert_eq!(amount_b_in, 2500);
    assert_eq!(new_amount_a, 2500);
    assert_eq!(new_amount_b, 2500);
}

#[test]
fn test_try_match_swapp_notes_arithmetic_case4() {
    let maker_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let taker_id = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();
    let matcher = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    let maker_note = create_partial_swap_note(
        maker_id,
        maker_id,
        FungibleAsset::new(faucet_a, 10).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 5).unwrap().into(),  // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // taker: 60 B  → 50 A
    let taker_note = create_partial_swap_note(
        taker_id,
        taker_id,
        FungibleAsset::new(faucet_b, 2).unwrap().into(), // offered
        FungibleAsset::new(faucet_a, 4).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // 2. Run the matcher
    // ────────────────────────────────────────────────────────────
    let swap = try_match_swapp_notes(&maker_note, &taker_note, matcher)
        .unwrap()
        .expect("orders should cross");

    // ────────────────────────────────────────────────────────────
    // 3. Assertions
    // ────────────────────────────────────────────────────────────
    // 3-a.  P2ID payloads
    let p2id_a_out = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    let p2id_b_out = swap
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    // 3-b.  Left-over maker order: 100 A → 60 B
    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("maker not 100 % filled");
    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();
}

#[test]
fn test_create_partial_swap_note_with_different_amounts() {
    let maker_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    // Maker offers 100 A, wants 80 B
    let maker_note = create_partial_swap_note(
        maker_id,
        maker_id,
        FungibleAsset::new(faucet_a, 100).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 80).unwrap().into(),  // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // Decompose the maker note and assert the amounts
    let (offered, wanted) = decompose_swapp_note(&maker_note).unwrap();
    assert_eq!(offered.amount(), 100);
    assert_eq!(offered.faucet_id(), faucet_a);
    assert_eq!(wanted.amount(), 80);
    assert_eq!(wanted.faucet_id(), faucet_b);
}

#[test]
fn test_price_to_swap_note_match() {
    let trader_1 = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let trader_2 = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let matcher_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    let swap_note_1 = price_to_swap_note(
        trader_1,
        trader_1,
        false,
        2490,
        2,
        &faucet_a,
        &faucet_b,
        Word::default(),
    );
    let swap_note_2 = price_to_swap_note(
        trader_2,
        trader_2,
        true,
        2500,
        1,
        &faucet_a,
        &faucet_b,
        Word::default(),
    );

    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_id).unwrap();

    println!("swap data: {:?}", swap_data);
}
