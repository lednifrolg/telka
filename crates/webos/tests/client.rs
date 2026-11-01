//! A registered connection against a fake TV over plain WebSocket.

mod common;

use std::time::Duration;

use common::{
    KEY, UUID, answer, drain, endpoint, greet, options, paired, quick, recv, send, timed, tv,
    welcome,
};
use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::timeout;
use webos::{Client, ClientKey, Error, Manifest};

#[tokio::test]
async fn connects_with_the_key_and_answers_requests() {
    let address = tv(|mut socket, _| async move {
        let register = welcome(&mut socket).await;
        assert_eq!(register["payload"]["client-key"], KEY);
        assert_eq!(
            register["payload"]["manifest"]["signed"]["appId"],
            "com.lge.test"
        );
        let request = recv(&mut socket).await.unwrap();
        assert_eq!(request["type"], "request");
        assert_eq!(request["uri"], "ssap://audio/getVolume");
        answer(
            &mut socket,
            &request,
            json!({"returnValue": true, "volumeStatus": {"volume": 12}}),
        )
        .await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    assert_eq!(client.hello().uuid, UUID);
    assert_eq!(client.hello().release.as_deref(), Some("6.5.3"));
    assert_eq!(client.paired(), &paired());
    let volume = client.request("audio/getVolume", json!({})).await.unwrap();
    assert_eq!(volume["volumeStatus"]["volume"], 12);
    client.close().await;
    assert!(client.is_closed());
}

#[tokio::test]
async fn another_tv_at_the_address_is_refused() {
    let address = tv(|mut socket, _| async move {
        greet(&mut socket, "another-tv").await;
        drain(&mut socket).await;
    })
    .await;
    let error = Client::connect(endpoint(address), &paired(), &options())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, Error::WrongTv { expected, found } if expected == UUID && found == "another-tv")
    );
}

#[tokio::test]
async fn an_unknown_key_brings_a_prompt_and_is_refused() {
    let (ended, seen) = oneshot::channel();
    let ended = std::sync::Mutex::new(Some(ended));
    let address = tv(move |mut socket, _| {
        let ended = ended.lock().unwrap().take();
        async move {
            greet(&mut socket, UUID).await;
            let register = recv(&mut socket).await.unwrap();
            send(&mut socket, json!({"type": "response", "id": register["id"], "payload": {"pairingType": "PROMPT", "returnValue": true}})).await;
            drain(&mut socket).await;
            let _ = ended.unwrap().send(());
        }
    })
    .await;
    let started = tokio::time::Instant::now();
    let error = Client::connect(endpoint(address), &paired(), &timed(quick()))
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::KeyRejected), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(1));
    // The connection ends, which takes the prompt off the TV's screen.
    timeout(Duration::from_secs(1), seen)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_key_the_tv_replaces_is_kept() {
    let address = tv(|mut socket, _| async move {
        greet(&mut socket, UUID).await;
        let register = recv(&mut socket).await.unwrap();
        send(&mut socket, json!({"type": "response", "id": register["id"], "payload": {"pairingType": "PROMPT", "returnValue": true}})).await;
        send(&mut socket, json!({"type": "registered", "id": register["id"], "payload": {"client-key": "a-new-key"}})).await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &timed(quick()))
        .await
        .unwrap();
    assert_eq!(client.paired().key, ClientKey::new("a-new-key"));
    assert_eq!(client.paired().manifest, Manifest::Signed);
}

#[tokio::test]
async fn refusals_come_back_as_errors() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        while let Some(request) = recv(&mut socket).await {
            let frame = match request["uri"].as_str().unwrap() {
                "ssap://com.webos.service.update/getCurrentSWInformation" => {
                    json!({"type": "error", "id": request["id"], "error": "401 insufficient permissions", "payload": {}})
                }
                "ssap://com.webos.media/getForegroundAppInfo" => {
                    json!({"type": "error", "id": request["id"], "error": "404 no such service or method", "payload": {}})
                }
                "ssap://settings/getSystemSettings" => json!({"type": "error", "id": request["id"], "error": "500 Application error",
                    "payload": {"returnValue": false, "errorCode": -1000, "errorText": "Some keys are not allowed for the request. ( pictureMode )"}}),
                _ => json!({"type": "response", "id": request["id"], "payload": {"returnValue": false, "errorText": "busy"}}),
            };
            send(&mut socket, frame).await;
        }
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    let denied = client
        .request(
            "com.webos.service.update/getCurrentSWInformation",
            json!({}),
        )
        .await;
    assert!(
        matches!(denied, Err(Error::Denied { message }) if message == "insufficient permissions")
    );
    let missing = client
        .request("com.webos.media/getForegroundAppInfo", json!({}))
        .await;
    assert!(
        matches!(missing, Err(Error::Unsupported { uri }) if uri == "ssap://com.webos.media/getForegroundAppInfo")
    );
    let refused = client
        .request(
            "settings/getSystemSettings",
            json!({"category": "picture", "keys": ["pictureMode"]}),
        )
        .await;
    assert!(matches!(refused, Err(Error::Rejected { message }) if message.contains("pictureMode")));
    let busy = client.request("system/turnOff", json!({})).await;
    assert!(matches!(busy, Err(Error::Rejected { message }) if message == "busy"));
    let frame = client
        .frame(
            "com.webos.service.update/getCurrentSWInformation",
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(frame["error"], "401 insufficient permissions");
}

#[tokio::test]
async fn answers_find_their_requests_in_any_order() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        let first = recv(&mut socket).await.unwrap();
        let second = recv(&mut socket).await.unwrap();
        answer(
            &mut socket,
            &second,
            json!({"returnValue": true, "uri": second["uri"]}),
        )
        .await;
        answer(
            &mut socket,
            &first,
            json!({"returnValue": true, "uri": first["uri"]}),
        )
        .await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    let (volume, power) = tokio::join!(
        client.request("audio/getVolume", json!({})),
        client.request("com.webos.service.tvpower/power/getPowerState", json!({})),
    );
    assert_eq!(volume.unwrap()["uri"], "ssap://audio/getVolume");
    assert_eq!(
        power.unwrap()["uri"],
        "ssap://com.webos.service.tvpower/power/getPowerState"
    );
}

#[tokio::test]
async fn subscriptions_deliver_changes_until_dropped() {
    let (unsubscribed, heard) = oneshot::channel();
    let unsubscribed = std::sync::Mutex::new(Some(unsubscribed));
    let address = tv(move |mut socket, _| {
        let unsubscribed = unsubscribed.lock().unwrap().take();
        async move {
            welcome(&mut socket).await;
            let subscribe = recv(&mut socket).await.unwrap();
            assert_eq!(subscribe["type"], "subscribe");
            for volume in [12, 13, 14] {
                answer(
                    &mut socket,
                    &subscribe,
                    json!({"returnValue": true, "volumeStatus": {"volume": volume}}),
                )
                .await;
            }
            let unsubscribe = recv(&mut socket).await.unwrap();
            assert_eq!(unsubscribe["type"], "unsubscribe");
            assert_eq!(unsubscribe["id"], subscribe["id"]);
            let _ = unsubscribed.unwrap().send(());
            drain(&mut socket).await;
        }
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    let mut volume = client
        .subscribe("audio/getVolume", json!({}))
        .await
        .unwrap();
    assert_eq!(volume.uri(), "ssap://audio/getVolume");
    assert_eq!(volume.current()["volumeStatus"]["volume"], 12);
    assert_eq!(
        volume.next().await.unwrap().unwrap()["volumeStatus"]["volume"],
        13
    );
    assert_eq!(
        volume.next().await.unwrap().unwrap()["volumeStatus"]["volume"],
        14
    );
    drop(volume);
    timeout(Duration::from_secs(2), heard)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_closed_connection_ends_requests_and_subscriptions() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        let subscribe = recv(&mut socket).await.unwrap();
        answer(
            &mut socket,
            &subscribe,
            json!({"returnValue": true, "state": "Active"}),
        )
        .await;
        let _unanswered = recv(&mut socket).await.unwrap();
        socket.close(None).await.unwrap();
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    let mut power = client
        .subscribe("com.webos.service.tvpower/power/getPowerState", json!({}))
        .await
        .unwrap();
    let request = client.request("system/getSystemInfo", json!({})).await;
    assert!(matches!(request, Err(Error::Closed)), "{request:?}");
    assert!(power.next().await.is_none());
    timeout(Duration::from_secs(1), client.closed())
        .await
        .unwrap();
    assert!(matches!(
        client.request("audio/getVolume", json!({})).await,
        Err(Error::Closed)
    ));
}

#[tokio::test]
async fn a_silent_tv_counts_as_gone() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        // Holding the socket without reading it: no pongs, no answers, like a TV gone to sleep.
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(socket);
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &timed(quick()))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), client.closed())
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unanswered_request_times_out_and_the_next_one_works() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        let _ignored = recv(&mut socket).await.unwrap();
        let second = recv(&mut socket).await.unwrap();
        answer(&mut socket, &second, json!({"returnValue": true})).await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &timed(quick()))
        .await
        .unwrap();
    let first = client
        .request("system.launcher/launch", json!({"id": "netflix"}))
        .await;
    assert!(matches!(first, Err(Error::TimedOut)), "{first:?}");
    assert!(client.request("audio/getVolume", json!({})).await.is_ok());
}

#[tokio::test]
async fn nothing_listens_on_a_closed_port() {
    let address = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let error = Client::connect(endpoint(address), &paired(), &options())
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::Connect(_)), "{error:?}");
}
