use miden_client::{Word, account::AccountId, note::Note};
use miden_clob::common::{price_to_swap_note, try_match_swapp_notes};
use miden_tx::utils::{Deserializable, Serializable};

#[test]
#[ignore]
fn test_price_to_swap_note_match() {
    let trader_1 = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let trader_2 = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();

    let matcher_id = AccountId::from_hex("0xa6511b8a76c05b1000009fdbdccce9").unwrap();
    let faucet_a = AccountId::from_hex("0xe125e96a9af535200000a5dc5d4500").unwrap();
    let faucet_b = AccountId::from_hex("0x34d1b6993361072000005737e7ae4b").unwrap();

    let swap_note_1: miden_client::note::Note = price_to_swap_note(
        trader_1,
        trader_1,
        false,
        2490,
        2,
        &faucet_a,
        &faucet_b,
        Word::default(),
    );
    let swap_note_2: miden_client::note::Note = price_to_swap_note(
        trader_2,
        trader_2,
        true,
        2500,
        1,
        &faucet_a,
        &faucet_b,
        Word::default(),
    );

    let swap_note_1_bytes: Vec<u8> = swap_note_1.to_bytes();
    let swap_note_2_bytes: Vec<u8> = swap_note_2.to_bytes();

    let note1_deserialized = Note::read_from_bytes(&swap_note_1_bytes);
    let note2_deserialized = Note::read_from_bytes(&swap_note_2_bytes);

    assert!(note1_deserialized.is_ok());
    assert!(note2_deserialized.is_ok());

    let _swap_data = try_match_swapp_notes(
        &note1_deserialized.unwrap(),
        &note2_deserialized.unwrap(),
        matcher_id,
    )
    .unwrap();
}
