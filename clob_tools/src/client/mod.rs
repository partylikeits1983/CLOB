use std::sync::Arc;
use tokio::time::{sleep, Duration};
use rand::rngs::StdRng;

use miden_client::{
    account::Account,
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{Note, NoteId, NoteTag},
    rpc::{Endpoint, GrpcClient},
    store::InputNoteRecord,
    Client, ClientError,
};

pub async fn wait_for_notes(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    account_id: &Account,
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
    client: &mut Client<FilesystemKeyStore<StdRng>>,
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

// Helper to instantiate Client
pub async fn instantiate_client(endpoint: Endpoint) -> Result<Client<FilesystemKeyStore<StdRng>>, ClientError> {
    let timeout_ms = 10_000;
    let rpc_api = Arc::new(GrpcClient::new(&endpoint, timeout_ms));
    let keystore_path = std::path::PathBuf::from("./keystore");
    let keystore = Arc::new(FilesystemKeyStore::<StdRng>::new(keystore_path).unwrap());

    let client = ClientBuilder::new()
        .rpc(rpc_api.clone())
        .authenticator(keystore.clone())
        .in_debug_mode(true.into())
        .build()
        .await?;

    Ok(client)
}

// Waits for note
pub async fn wait_for_note(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    _account_id: &Account,
    expected: &Note,
) -> Result<(), ClientError> {
    loop {
        client.sync_state().await?;

        let notes: Vec<(
            InputNoteRecord,
            Vec<(
                miden_client::account::AccountId,
                miden_client::note::NoteRelevance,
            )>,
        )> = client.get_consumable_notes(None).await?;

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
