//! Blob Storage tests against a real storage account.
//!
//! The user delegation SAS is signed by hand from a documented string layout,
//! so nothing but a real round trip proves it correct: a wrong field order or a
//! mis-encoded character produces a signature Azure rejects, which no unit test
//! could catch.
//!
//! Runs only when `MEDIAGENERATOR_TEST_STORAGE_ACCOUNT` is set. The signed-in
//! identity needs **Storage Blob Data Contributor** on the account — the
//! control-plane roles (Owner, Contributor) do not grant data access.

use std::time::Duration;

use mediagenerator_storage::BlobStore;
use uuid::Uuid;

/// Environment variable naming the storage account to test against.
const ACCOUNT_VAR: &str = "MEDIAGENERATOR_TEST_STORAGE_ACCOUNT";

/// Environment variable naming the container. Defaults to `media`.
const CONTAINER_VAR: &str = "MEDIAGENERATOR_TEST_CONTAINER";

/// Builds a store, or returns `None` when unconfigured.
fn store() -> Option<BlobStore> {
    let account = std::env::var(ACCOUNT_VAR).ok()?;
    let container = std::env::var(CONTAINER_VAR).unwrap_or_else(|_| "media".to_owned());

    match BlobStore::new(&account, &container) {
        Ok(store) => Some(store),
        Err(error) => panic!("{ACCOUNT_VAR} is set but no credential is usable: {error}"),
    }
}

#[tokio::test]
async fn media_survives_a_round_trip_through_a_signed_link() {
    let Some(store) = store() else {
        eprintln!("skipped: {ACCOUNT_VAR} is not set");
        return;
    };

    let path = format!("test/{}.txt", Uuid::new_v4());
    let body = "Solnedgang over Oslofjorden — æøå".as_bytes().to_vec();

    store
        .upload(&path, "text/plain; charset=utf-8", body.clone())
        .await
        .expect("upload should succeed");

    let url = store
        .read_url(&path, Duration::from_secs(300), None)
        .await
        .expect("a SAS should be issued");

    // Deliberately a plain client with no credentials: the link has to stand on
    // its own, which is the whole point of handing it to a browser.
    let fetched = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .expect("the signed link should be reachable");

    assert!(
        fetched.status().is_success(),
        "signed link was rejected with {}: {}",
        fetched.status(),
        fetched.text().await.unwrap_or_default()
    );

    let returned = fetched.bytes().await.expect("body should be readable");
    assert_eq!(returned.as_ref(), body.as_slice());

    store.delete(&path).await.expect("cleanup should succeed");
}

#[tokio::test]
async fn the_container_is_not_readable_without_a_signature() {
    let Some(store) = store() else {
        eprintln!("skipped: {ACCOUNT_VAR} is not set");
        return;
    };

    let path = format!("test/{}.txt", Uuid::new_v4());
    store
        .upload(&path, "text/plain", b"hemmelig".to_vec())
        .await
        .expect("upload should succeed");

    // The same URL without the query string. If this succeeds, the container is
    // public and every generated file is world-readable.
    let url = store
        .read_url(&path, Duration::from_secs(300), None)
        .await
        .expect("a SAS should be issued");
    let unsigned = url.split('?').next().expect("a URL before the query");

    let response = reqwest::Client::new()
        .get(unsigned)
        .send()
        .await
        .expect("the request should complete");

    assert!(
        !response.status().is_success(),
        "the container must not be publicly readable"
    );

    store.delete(&path).await.expect("cleanup should succeed");
}

#[tokio::test]
async fn an_expired_link_stops_working() {
    let Some(store) = store() else {
        eprintln!("skipped: {ACCOUNT_VAR} is not set");
        return;
    };

    let path = format!("test/{}.txt", Uuid::new_v4());
    store
        .upload(&path, "text/plain", b"utlopt".to_vec())
        .await
        .expect("upload should succeed");

    // One second, then wait it out. This is what makes SAS_TTL_MINUTES a real
    // control rather than a setting nobody has checked.
    let url = store
        .read_url(&path, Duration::from_secs(1), None)
        .await
        .expect("a SAS should be issued");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .expect("the request should complete");

    assert!(
        !response.status().is_success(),
        "an expired signature must be refused, got {}",
        response.status()
    );

    store.delete(&path).await.expect("cleanup should succeed");
}

#[tokio::test]
async fn deleting_a_missing_blob_is_not_an_error() {
    let Some(store) = store() else {
        eprintln!("skipped: {ACCOUNT_VAR} is not set");
        return;
    };

    // Retention runs repeatedly over the same rows; a blob already gone must
    // not fail the sweep.
    let path = format!("test/{}-finnes-ikke.txt", Uuid::new_v4());
    store
        .delete(&path)
        .await
        .expect("delete should tolerate a missing blob");
}

#[tokio::test]
async fn a_download_link_tells_the_browser_to_save_the_file() {
    let Some(store) = store() else {
        eprintln!("skipped: {ACCOUNT_VAR} is not set");
        return;
    };

    let path = format!("test/{}.txt", Uuid::new_v4());
    store
        .upload(&path, "text/plain", b"last ned meg".to_vec())
        .await
        .expect("upload should succeed");

    // The `download` attribute on a link is ignored cross-origin, and the SAS
    // points at another host. Content-Disposition in the signature is what
    // actually makes the browser save rather than navigate — and it is signed,
    // so it cannot be bolted onto the query afterwards.
    let url = store
        .read_url(&path, Duration::from_secs(300), Some("bilde-1.png"))
        .await
        .expect("a SAS should be issued");

    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .expect("the signed link should be reachable");

    assert!(
        response.status().is_success(),
        "signed link was rejected with {}",
        response.status()
    );

    let disposition = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    assert!(
        disposition.starts_with("attachment"),
        "expected an attachment, got {disposition:?}"
    );
    assert!(disposition.contains("bilde-1.png"), "got {disposition:?}");

    store.delete(&path).await.expect("cleanup should succeed");
}
