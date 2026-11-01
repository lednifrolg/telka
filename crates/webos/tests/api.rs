//! The typed API against a fake TV: reports, commands, the input socket and the alert trick.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::{answer, drain, endpoint, options, paired, quick, recv, timed, tv, welcome};
use futures_util::StreamExt;
use serde_json::json;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use webos::{Button, Client, Error, PowerMode, PowerState, Volume};

#[tokio::test]
async fn reports_are_read_and_followed() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        let get = recv(&mut socket).await.unwrap();
        assert_eq!(get["uri"], "ssap://com.webos.service.tvpower/power/getPowerState");
        answer(&mut socket, &get, json!({"returnValue": true, "state": "Screen Off"})).await;
        let watch = recv(&mut socket).await.unwrap();
        assert_eq!(watch["type"], "subscribe");
        assert_eq!(watch["uri"], "ssap://audio/getVolume");
        let status = |volume: u8, mute: bool| json!({"returnValue": true, "volumeStatus": {"volume": volume, "muteStatus": mute, "maxVolume": 100}});
        answer(&mut socket, &watch, status(12, false)).await;
        answer(&mut socket, &watch, status(13, false)).await;
        answer(&mut socket, &watch, json!({"returnValue": true, "nonsense": true})).await;
        answer(&mut socket, &watch, status(13, true)).await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    let power: PowerState = client.get().await.unwrap();
    assert_eq!(power.mode, PowerMode::ScreenOff);
    let mut volume = client.watch::<Volume>().await.unwrap();
    assert_eq!(volume.current().level, 12);
    assert_eq!(volume.next().await.unwrap().unwrap().level, 13);
    assert!(matches!(
        volume.next().await.unwrap(),
        Err(Error::Decode { .. })
    ));
    assert!(volume.next().await.unwrap().unwrap().muted);
}

#[tokio::test]
async fn turning_off_asks_the_state_first() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        for state in ["Active Standby", "Active"] {
            let get = recv(&mut socket).await.unwrap();
            assert_eq!(
                get["uri"],
                "ssap://com.webos.service.tvpower/power/getPowerState"
            );
            answer(
                &mut socket,
                &get,
                json!({"returnValue": true, "state": state}),
            )
            .await;
        }
        let off = recv(&mut socket).await.unwrap();
        assert_eq!(off["uri"], "ssap://system/turnOff");
        answer(&mut socket, &off, json!({"returnValue": true})).await;
        let screen = recv(&mut socket).await.unwrap();
        assert_eq!(
            screen["uri"],
            "ssap://com.webos.service.tvpower/power/turnOffScreen"
        );
        assert_eq!(screen["payload"], json!({"standbyMode": "active"}));
        answer(&mut socket, &screen, json!({"returnValue": true})).await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    assert!(
        !client.turn_off().await.unwrap(),
        "Active Standby: nothing sent"
    );
    assert!(client.turn_off().await.unwrap());
    client.set_screen(false).await.unwrap();
}

#[tokio::test]
async fn commands_send_what_the_tv_expects() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        let expected = [
            ("ssap://system.launcher/launch", json!({"id": "youtube.leanback.v4", "contentId": "https://www.youtube.com/watch?v=x", "params": {"contentTarget": "https://www.youtube.com/watch?v=x"}})),
            ("ssap://system.launcher/launch", json!({"id": "netflix"})),
            ("ssap://tv/switchInput", json!({"inputId": "HDMI_3"})),
            ("ssap://audio/setVolume", json!({"volume": 15})),
            ("ssap://audio/setMute", json!({"mute": true})),
            ("ssap://media.controls/pause", json!({})),
            ("ssap://com.webos.service.ime/insertText", json!({"text": "breaking bad", "replace": false})),
            ("ssap://com.webos.service.ime/sendEnterKey", json!({})),
            ("ssap://system.notifications/createToast", json!({"message": "Laundry is done"})),
        ];
        for (uri, payload) in expected {
            let request = recv(&mut socket).await.unwrap();
            assert_eq!(request["uri"], uri);
            assert_eq!(request["payload"], payload, "{uri}");
            answer(&mut socket, &request, json!({"returnValue": true, "toastId": "toast-1"})).await;
        }
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    client
        .launch(
            "youtube.leanback.v4",
            Some("https://www.youtube.com/watch?v=x"),
        )
        .await
        .unwrap();
    client.launch("netflix", None).await.unwrap();
    client.switch_input("HDMI_3").await.unwrap();
    client.set_volume(15).await.unwrap();
    client.set_mute(true).await.unwrap();
    client.media(webos::Media::Pause).await.unwrap();
    client.insert_text("breaking bad", false).await.unwrap();
    client.send_enter().await.unwrap();
    assert_eq!(
        client.toast("Laundry is done").await.unwrap().as_deref(),
        Some("toast-1")
    );
}

#[tokio::test]
async fn the_remote_sends_buttons_and_pointer_frames() {
    let port: Arc<OnceLock<u16>> = Arc::default();
    let (frames, mut received) = mpsc::unbounded_channel();
    let frames = Arc::new(Mutex::new(Some(frames)));
    let address = tv({
        let port = Arc::clone(&port);
        move |mut socket, connection| {
            let port = Arc::clone(&port);
            // Only the input socket, the second connection, records frames.
            let frames = (connection == 1)
                .then(|| frames.lock().unwrap().take())
                .flatten();
            async move {
                if connection == 0 {
                    welcome(&mut socket).await;
                    let request = recv(&mut socket).await.unwrap();
                    assert_eq!(
                        request["uri"],
                        "ssap://com.webos.service.networkinput/getPointerInputSocket"
                    );
                    let path = format!(
                        "ws://192.0.2.1:{}/resources/0123/netinput.pointer.sock",
                        port.get().unwrap()
                    );
                    answer(
                        &mut socket,
                        &request,
                        json!({"returnValue": true, "socketPath": path}),
                    )
                    .await;
                    drain(&mut socket).await;
                } else if let Some(frames) = frames {
                    while let Some(Ok(message)) = socket.next().await {
                        if let Message::Text(text) = message {
                            let _ = frames.send(text.to_string());
                        }
                    }
                }
            }
        }
    })
    .await;
    port.set(address.port()).unwrap();
    let client = Client::connect(endpoint(address), &paired(), &options())
        .await
        .unwrap();
    // The socket path names another host; the remote still connects to this TV.
    let remote = client.remote().await.unwrap();
    remote.button(Button::HOME).await.unwrap();
    remote
        .button(Button::from_name("down").unwrap())
        .await
        .unwrap();
    remote.move_by(12, -4, false).await.unwrap();
    remote.click().await.unwrap();
    remote.scroll(0, -3).await.unwrap();
    remote.close().await;
    let mut got = Vec::new();
    while let Ok(Some(frame)) = timeout(Duration::from_secs(1), received.recv()).await {
        got.push(frame);
    }
    assert_eq!(
        got,
        [
            "type:button\nname:HOME\n\n",
            "type:button\nname:DOWN\n\n",
            "type:move\ndx:12\ndy:-4\ndown:0\n\n",
            "type:click\n\n",
            "type:scroll\ndx:0\ndy:-3\n\n",
        ]
    );
}

#[tokio::test]
async fn luna_calls_go_through_an_alert() {
    let address = tv(|mut socket, _| async move {
        welcome(&mut socket).await;
        let create = recv(&mut socket).await.unwrap();
        assert_eq!(create["uri"], "ssap://system.notifications/createAlert");
        let target = "luna://com.webos.settingsservice/setSystemSettings";
        let params = json!({"category": "picture", "settings": {"backlight": "75"}});
        assert_eq!(
            create["payload"]["onclose"],
            json!({"uri": target, "params": params})
        );
        assert_eq!(
            create["payload"]["onfail"],
            json!({"uri": target, "params": params})
        );
        assert_eq!(create["payload"]["buttons"][0]["onClick"], target);
        answer(
            &mut socket,
            &create,
            json!({"returnValue": true, "alertId": "alert-1"}),
        )
        .await;
        let close = recv(&mut socket).await.unwrap();
        assert_eq!(close["uri"], "ssap://system.notifications/closeAlert");
        assert_eq!(close["payload"], json!({"alertId": "alert-1"}));
        answer(&mut socket, &close, json!({"returnValue": true})).await;
        // A second alert that never closes, like one stuck on screen.
        let create = recv(&mut socket).await.unwrap();
        answer(
            &mut socket,
            &create,
            json!({"returnValue": true, "alertId": "alert-2"}),
        )
        .await;
        drain(&mut socket).await;
    })
    .await;
    let client = Client::connect(endpoint(address), &paired(), &timed(quick()))
        .await
        .unwrap();
    client
        .set_system_settings("picture", json!({"backlight": "75"}))
        .await
        .unwrap();
    let stuck = client
        .set_system_settings("time", json!({"sleepTimer": "30"}))
        .await;
    assert!(matches!(stuck, Err(Error::TimedOut)), "{stuck:?}");
}

#[tokio::test]
async fn wake_on_lan_sends_the_magic_packet_three_times() {
    let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let second = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let targets: Vec<SocketAddr> = vec![first.local_addr().unwrap(), second.local_addr().unwrap()];
    let mac = "02:00:5e:10:00:01".parse().unwrap();
    webos::wake_to(mac, &targets).await.unwrap();
    for socket in [first, second] {
        let mut buffer = [0; 200];
        for _ in 0..3 {
            let (length, _) = timeout(Duration::from_secs(1), socket.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(buffer[..length], webos::magic_packet(mac));
        }
    }
}
