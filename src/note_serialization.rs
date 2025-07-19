use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use miden_client::note::Note;
use miden_tx::utils::{Deserializable, Serializable};

pub fn serialize_note(note: &Note) -> Result<String> {
    // Use the proper Miden serialization method
    let note_bytes: Vec<u8> = note.to_bytes();
    let encoded = general_purpose::STANDARD.encode(note_bytes);
    Ok(encoded)
}

pub fn deserialize_note(encoded: &str) -> Result<Note> {
    // Decode from base64
    let note_bytes = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| anyhow!("Failed to decode base64: {}", e))?;

    // Deserialize the note from bytes using the proper method
    let note = Note::read_from_bytes(&note_bytes)
        .map_err(|e| anyhow!("Failed to deserialize note: {}", e))?;

    Ok(note)
}

// Helper function to extract key information from a note for database storage
pub fn extract_note_info(note: &Note) -> Result<(String, String, u64, String, u64, f64, bool)> {
    use crate::common::{creator_of, decompose_swapp_note};

    let (offered, requested) = decompose_swapp_note(note)
        .map_err(|e| anyhow!("Failed to decompose swap note: {:?}", e))?;

    let creator = creator_of(note);

    // Calculate price and determine if it's a bid
    let price = if offered.amount() > 0 {
        requested.amount() as f64 / offered.amount() as f64
    } else {
        0.0
    };

    // Determine if this is a bid or ask based on the actual asset types
    // Load environment variables to get the faucet IDs
    use dotenv::dotenv;
    use std::env;
    dotenv().ok();

    let usdc_faucet_id = env::var("USDC_FAUCET_ID").unwrap_or_default();
    let _eth_faucet_id = env::var("ETH_FAUCET_ID").unwrap_or_default();

    // Parse faucet IDs to AccountId
    let usdc_id = if !usdc_faucet_id.is_empty() {
        miden_client::account::AccountId::from_hex(&usdc_faucet_id).ok()
    } else {
        None
    };

    // Determine bid vs ask based on what is being offered:
    // - BID (buying ETH): offers USDC, wants ETH
    // - ASK (selling ETH): offers ETH, wants USDC
    let is_bid = if let Some(usdc_id) = usdc_id {
        offered.faucet_id() == usdc_id
    } else {
        // Fallback: assume the first faucet is USDC if we can't load from env
        // This is a heuristic that could be improved with a faucet registry
        offered.faucet_id().to_hex() < requested.faucet_id().to_hex()
    };

    Ok((
        creator.to_hex(),
        offered.faucet_id().to_hex(),
        offered.amount(),
        requested.faucet_id().to_hex(),
        requested.amount(),
        price,
        is_bid,
    ))
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        // This test would need actual Miden note objects to work properly
        // For now it's a placeholder
    }
}
