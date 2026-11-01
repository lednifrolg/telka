//! Pairing against a fake TV: the prompt, the fallback to the unsigned manifest, and refusals.

mod common;

use std::time::Duration;

use common::{KEY, UUID, answer, drain, endpoint, greet, options, recv, send, tv};
use serde_json::json;
use webos::{ClientKey, Error, Manifest, Pairing};

/// Shows the prompt for the register frame.
async fn prompt(socket: &mut common::Socket) -> serde_json::Value {
    let register = recv(socket).await.unwrap();
    assert_eq!(register["type"], "register");
    assert!(register["payload"].get("client-key").is_none());
    send(
        socket,
        json!({"type": "response", "id": register["id"], "payload": {"pairingType": "PROMPT", "returnValue": true}}),
    )
    .await;
    register
}

#[tokio::test]
async fn pairing_waits_for_the_prompt_and_stays_connected() {
    let address = tv(|mut socket, _| async move {
        greet(&mut socket, UUID).await;
        let register = prompt(&mut socket).await;
        assert_eq!(
            register["payload"]["manifest"]["signed"]["vendorId"],
            "com.lge"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        send(
            &mut socket,
            json!({"type": "registered", "id": register["id"], "payload": {"client-key": KEY}}),
        )
        .await;
        let request = recv(&mut socket).await.unwrap();
        answer(
            &mut socket,
            &request,
            json!({"returnValue": true, "state": "Active"}),
        )
        .await;
        drain(&mut socket).await;
    })
    .await;
    let pairing = Pairing::start(endpoint(address), &options()).await.unwrap();
    assert_eq!(pairing.manifest(), Manifest::Signed);
    assert_eq!(pairing.hello().uuid, UUID);
    let client = pairing.finish(Duration::from_secs(5)).await.unwrap();
    assert_eq!(client.paired().key, ClientKey::new(KEY));
    assert_eq!(client.paired().uuid, UUID);
    let power = client
        .request("com.webos.service.tvpower/power/getPowerState", json!({}))
        .await
        .unwrap();
    assert_eq!(power["state"], "Active");
}

#[tokio::test]
async fn a_refused_signed_manifest_falls_back_to_the_unsigned_one() {
    let address = tv(|mut socket, connection| async move {
        greet(&mut socket, UUID).await;
        if connection == 0 {
            let register = recv(&mut socket).await.unwrap();
            assert!(register["payload"]["manifest"].get("signed").is_some());
            send(
                &mut socket,
                json!({"type": "error", "id": register["id"],
                "error": "403 Pairing rejected: blacklisted certificate detected", "payload": {}}),
            )
            .await;
            drain(&mut socket).await;
            return;
        }
        let register = prompt(&mut socket).await;
        assert!(register["payload"]["manifest"].get("signed").is_none());
        send(
            &mut socket,
            json!({"type": "registered", "id": register["id"], "payload": {"client-key": KEY}}),
        )
        .await;
        drain(&mut socket).await;
    })
    .await;
    let pairing = Pairing::start(endpoint(address), &options()).await.unwrap();
    assert_eq!(pairing.manifest(), Manifest::Unsigned);
    let client = pairing.finish(Duration::from_secs(5)).await.unwrap();
    assert_eq!(client.paired().manifest, Manifest::Unsigned);
}

#[tokio::test]
async fn a_declined_prompt_ends_the_pairing() {
    let address = tv(|mut socket, _| async move {
        greet(&mut socket, UUID).await;
        let register = prompt(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "error", "id": register["id"], "error": "403 cancelled", "payload": {}}),
        )
        .await;
        drain(&mut socket).await;
    })
    .await;
    let pairing = Pairing::start(endpoint(address), &options()).await.unwrap();
    let error = pairing.finish(Duration::from_secs(5)).await.err().unwrap();
    assert!(matches!(error, Error::PairingDeclined), "{error:?}");
}

#[tokio::test]
async fn an_unanswered_prompt_times_out_and_closes() {
    let (closed, seen) = tokio::sync::oneshot::channel();
    let closed = std::sync::Mutex::new(Some(closed));
    let address = tv(move |mut socket, _| {
        let closed = closed.lock().unwrap().take();
        async move {
            greet(&mut socket, UUID).await;
            prompt(&mut socket).await;
            drain(&mut socket).await;
            if let Some(closed) = closed {
                let _ = closed.send(());
            }
        }
    })
    .await;
    let pairing = Pairing::start(endpoint(address), &options()).await.unwrap();
    let error = pairing
        .finish(Duration::from_millis(200))
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::PairingTimedOut), "{error:?}");
    tokio::time::timeout(Duration::from_secs(2), seen)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn hello_identifies_the_tv_without_pairing() {
    let address = tv(|mut socket, _| async move {
        greet(&mut socket, UUID).await;
        drain(&mut socket).await;
    })
    .await;
    let hello = webos::hello(endpoint(address), &options()).await.unwrap();
    assert_eq!(hello.uuid, UUID);
    assert_eq!(hello.pairing_types, ["PIN", "PROMPT", "COMBINED"]);
    assert_eq!(hello.raw["payload"]["deviceOS"], "webOS");
}
