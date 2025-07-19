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
