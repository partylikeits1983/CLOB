use miden_client::{account::AccountId, asset::FungibleAsset, Word};
use miden_clob::{
    common::{
        create_partial_swap_note, decompose_swapp_note, price_to_swap_note, try_match_swapp_notes,
    },
    compute_partial_swapp, create_order_simple_testing,
};

#[test]
#[ignore]
fn test_try_match_swapp_notes_arithmetic() {
    // ────────────────────────────────────────────────────────────
    // Test case from the error message:
    // Note 1: offers 10, wants 45290 (ratio: 4529)
    // Note 2: offers 54360, wants 12 (ratio: 0.00022)
    // Note 2 has a much better price and should be the maker (partially filled)
    // ────────────────────────────────────────────────────────────

    let note1_creator = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let note2_creator = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();
    let matcher = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();

    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    // Note 1: Small order - offers 10 B, wants 45290 A
    let note1 = create_partial_swap_note(
        note1_creator,
        note1_creator,
        FungibleAsset::new(faucet_b, 10).unwrap().into(), // offered
        FungibleAsset::new(faucet_a, 45290).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // Note 2: Large order - offers 54360 A, wants 12 B
    let note2 = create_partial_swap_note(
        note2_creator,
        note2_creator,
        FungibleAsset::new(faucet_a, 54360).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 12).unwrap().into(),    // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // ────────────────────────────────────────────────────────────
    // Run the matcher - should handle the order correctly regardless of input order
    // ────────────────────────────────────────────────────────────
    let swap = try_match_swapp_notes(&note1, &note2, matcher)
        .unwrap()
        .expect("orders should cross");

    // ────────────────────────────────────────────────────────────
    // Assertions
    // ────────────────────────────────────────────────────────────

    // The naming convention is confusing, but based on the actual output:
    // p2id_from_1_to_2 contains what note1's creator receives
    // p2id_from_2_to_1 contains what note2's creator receives

    let p2id_from_1_to_2 = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    let p2id_from_2_to_1 = swap
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();

    // Note1 is fully filled: offers 10 B, receives 45290 A
    // So p2id_from_1_to_2 should contain 45290 A (what note1 creator receives)
    assert_eq!(
        p2id_from_1_to_2.amount(),
        45290,
        "Note1 creator should receive 45290 A"
    );
    assert_eq!(p2id_from_1_to_2.faucet_id(), faucet_a);

    // Note2 is partially filled: offers 54360 A, receives 10 B
    // So p2id_from_2_to_1 should contain 10 B (what note2 creator receives)
    assert_eq!(
        p2id_from_2_to_1.amount(),
        10,
        "Note2 creator should receive 10 B"
    );
    assert_eq!(p2id_from_2_to_1.faucet_id(), faucet_b);

    // Note2 should have leftover (it's the maker, partially filled)
    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("Note2 (maker) should not be 100% filled");

    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();

    // Leftover calculation:
    // Note2 originally offered 54360 A and wanted 12 B
    // It gave away 45290 A (to fill note1 completely)
    // Leftover: 54360 - 45290 = 9070 A
    // It received 10 B (all that note1 offered)
    // Still wants: 12 - 10 = 2 B
    assert_eq!(left_off.amount(), 9070, "Leftover should offer 9070 A");
    assert_eq!(left_off.faucet_id(), faucet_a);
    assert_eq!(left_req.amount(), 2, "Leftover should still want 2 B");
    assert_eq!(left_req.faucet_id(), faucet_b);

    // Note arguments - these represent what each note receives
    assert_eq!(
        swap.note1_args[3].as_int(),
        45290,
        "Note1 arg: receives 45290 A"
    );
    assert_eq!(swap.note2_args[3].as_int(), 10, "Note2 arg: receives 10 B");
}

#[test]
#[ignore]
fn test_try_match_swapp_notes_arithmetic_case2() {
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
    let p2id_a_out = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_a_out.amount(), 60);
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

    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("maker not 100 % filled");
    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();
    assert_eq!(left_off.amount(), 50);
    assert_eq!(left_off.faucet_id(), faucet_a);
    assert_eq!(left_req.amount(), 30);
    assert_eq!(left_req.faucet_id(), faucet_b);

    assert_eq!(swap.note1_args[3].as_int(), 60); // maker receives 30 B
    assert_eq!(swap.note2_args[3].as_int(), 50); // taker receives 50 A
}

#[test]
#[ignore]
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
    let p2id_a_out = swap
        .p2id_from_1_to_2
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_a_out.amount(), 352);
    assert_eq!(p2id_a_out.faucet_id(), faucet_b);

    let p2id_b_out = swap
        .p2id_from_2_to_1
        .assets()
        .iter()
        .next()
        .unwrap()
        .unwrap_fungible();
    assert_eq!(p2id_b_out.amount(), 912736);
    assert_eq!(p2id_b_out.faucet_id(), faucet_a);

    let leftover = swap
        .leftover_swapp_note
        .as_ref()
        .expect("maker not 100 % filled");
    let (left_off, left_req) = decompose_swapp_note(leftover).unwrap();
    assert_eq!(left_off.amount(), 887995);
    assert_eq!(left_off.faucet_id(), faucet_a);
    assert_eq!(left_req.amount(), 337);
    assert_eq!(left_req.faucet_id(), faucet_b);

    assert_eq!(swap.note1_args[3].as_int(), 352);
    assert_eq!(swap.note2_args[3].as_int(), 912736);
}

#[tokio::test]
#[ignore]
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

#[tokio::test]
#[ignore]
async fn test_compute_partial_swapp_edge_case() {
    let amount_b_in = 522;
    let (amount_a_1, new_amount_a, new_amount_b) = compute_partial_swapp(
        438,         // originally offered A
        1081860,     // originally requested B
        amount_b_in, // Bob fills 25 B
    );

    println!("==== test_compute_partial_swapp ====");
    println!("amount_a_1 (A out):          {}", amount_a_1);
    println!("amount_b_1 (B in):           {}", amount_b_in);
    println!("new_amount_a (A leftover):   {}", new_amount_a);
    println!("new_amount_b (B leftover):   {}", new_amount_b);

    assert_eq!(amount_a_1, 1368162);
    assert_eq!(new_amount_a, 0);
    assert_eq!(new_amount_b, 0);
}

#[test]
#[ignore]
fn test_try_match_swapp_notes_arithmetic_case4() {
    let maker_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let taker_id = AccountId::from_hex("0xbbb871c76d7a27100000cc8533e494").unwrap();
    let matcher = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    let maker_note = create_partial_swap_note(
        maker_id,
        maker_id,
        FungibleAsset::new(faucet_a, 600).unwrap().into(), // offered
        FungibleAsset::new(faucet_b, 1455600).unwrap().into(), // wanted
        Word::default(),
        0,
    )
    .unwrap();

    // taker: 60 B  → 50 A
    let taker_note = create_partial_swap_note(
        taker_id,
        taker_id,
        FungibleAsset::new(faucet_b, 173737).unwrap().into(), // offered
        FungibleAsset::new(faucet_a, 71).unwrap().into(),     // wanted
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
#[ignore]
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
#[ignore]
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

#[test]
#[ignore]
fn match_swap_notes_algo_test() {
    let trader_1 = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let trader_2 = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let matcher_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    // ────────────────────────────────────────────────────────────
    // 2.  Trader-1:
    // ────────────────────────────────────────────────────────────
    let swap_note_1 = create_order_simple_testing(
        trader_1,
        FungibleAsset::new(faucet_a, 438).unwrap().into(),
        FungibleAsset::new(faucet_b, 1081860).unwrap().into(),
    );

    // ────────────────────────────────────────────────────────────
    // 3.  Trader-2:
    // ────────────────────────────────────────────────────────────
    let swap_note_2 = create_order_simple_testing(
        trader_2,
        FungibleAsset::new(faucet_b, 1120588).unwrap().into(), // offered (use exact amount from working match)
        FungibleAsset::new(faucet_a, 434).unwrap().into(), // wanted (use exact amount from working match)
    );

    let swap_data = try_match_swapp_notes(&swap_note_1, &swap_note_2, matcher_id).unwrap();

    println!(
        "swap {:?}",
        swap_data.unwrap().p2id_from_1_to_2.script().root()
    );

    // println!("swap data: {:?}", swap_data);
}
