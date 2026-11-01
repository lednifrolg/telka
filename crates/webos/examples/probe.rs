//! Live checks against a real TV while the `telka` CLI does not exist yet.
//!
//! Every command except `hello` and `pair` reads the TV from the JSON file that `WEBOS_PROBE_TV`
//! names: what `pair` writes (`address`, `uuid`, `key`, `manifest`), or an older file with only
//! `host` and `client_key`, which counts as an unsigned pairing. `capture` only reads; `request`
//! sends whatever it is given, so it can change the TV. `WEBOS_PROBE_PLAIN` set to anything
//! connects over the plain port only.
//!
//! ```sh
//! cargo run -p webos --example probe -- hello 192.168.1.30
//! cargo run -p webos --example probe -- pair 192.168.1.30 tv.json
//! WEBOS_PROBE_TV=tv.json cargo run -p webos --example probe -- capture research/fixtures
//! WEBOS_PROBE_TV=tv.json cargo run -p webos --example probe -- request audio/getVolume
//! WEBOS_PROBE_TV=tv.json cargo run -p webos --example probe -- watch 60 audio/getVolume
//! ```

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webos::{
    Button, Client, ClientKey, Endpoint, ForegroundApp, Inputs, Interfaces, LaunchPoints,
    LinkStatus, MacAddress, Manifest, Options, PROMPT_WINDOW, Paired, Pairing, Picture, PowerState,
    SoundOutput, Volume, WakeOnLan,
};

const USAGE: &str = "usage: probe hello <ip>
       probe pair <ip> <out-file> [signed|unsigned]
       probe capture <out-dir>
       probe request <uri> [payload-json]
       probe watch <seconds> <uri>...
       probe state
       probe discover [seconds]
       probe key <button>...
       probe wake
       probe luna <uri> <params-json>";

type Result<T = (), E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

/// A paired TV, as `pair` writes it.
#[derive(Serialize, Deserialize)]
struct Tv {
    #[serde(alias = "host")]
    address: IpAddr,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(alias = "client_key")]
    key: ClientKey,
    #[serde(default = "unsigned")]
    manifest: Manifest,
    /// The interface to wake, from pairing.
    #[serde(default)]
    mac: Option<MacAddress>,
}

fn unsigned() -> Manifest {
    Manifest::Unsigned
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[&str]) -> Result {
    let options = Options::lg()?;
    match args {
        ["hello", ip] => hello(ip.parse()?, &options).await,
        ["pair", ip, out] => pair(ip.parse()?, Path::new(out), None, &options).await,
        ["pair", ip, out, manifest] => {
            let manifest = serde_json::from_value(json!(manifest))?;
            pair(ip.parse()?, Path::new(out), Some(manifest), &options).await
        }
        ["capture", out] => capture(&connect(&options).await?, Path::new(out)).await,
        ["request", uri] => request(&connect(&options).await?, uri, json!({})).await,
        ["request", uri, payload] => {
            request(
                &connect(&options).await?,
                uri,
                serde_json::from_str(payload)?,
            )
            .await
        }
        ["watch", seconds, uris @ ..] if !uris.is_empty() => {
            watch(&connect(&options).await?, seconds.parse()?, uris).await
        }
        ["state"] => state(&connect(&options).await?).await,
        ["discover"] => discover(3).await,
        ["discover", seconds] => discover(seconds.parse()?).await,
        ["key", buttons @ ..] if !buttons.is_empty() => {
            keys(&connect(&options).await?, buttons).await
        }
        ["wake"] => wake().await,
        ["luna", uri, params] => {
            let client = connect(&options).await?;
            let started = Instant::now();
            client.luna(uri, serde_json::from_str(params)?).await?;
            println!("alert closed in {:.2} s", started.elapsed().as_secs_f64());
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

/// The TLS port first, then the plain one, like `Client::connect_to`.
async fn hello(ip: IpAddr, options: &Options) -> Result {
    for endpoint in [Endpoint::secure(ip), Endpoint::plain(ip)] {
        let started = Instant::now();
        match webos::hello(endpoint, options).await {
            Ok(hello) => {
                println!(
                    "{}  {:.2} s",
                    endpoint.url(),
                    started.elapsed().as_secs_f64()
                );
                println!("{}", serde_json::to_string_pretty(&hello.raw)?);
                println!("certificate {:?}", options.tls.seen());
            }
            Err(error) => println!("{}  {error}", endpoint.url()),
        }
    }
    Ok(())
}

async fn pair(ip: IpAddr, out: &Path, manifest: Option<Manifest>, options: &Options) -> Result {
    let pairing = match manifest {
        Some(manifest) => Pairing::start_with(Endpoint::secure(ip), manifest, options).await?,
        None => Pairing::start(Endpoint::secure(ip), options).await?,
    };
    println!(
        "{:?} manifest; accept the prompt on the TV within {} s",
        pairing.manifest(),
        PROMPT_WINDOW.as_secs()
    );
    let client = pairing.finish(PROMPT_WINDOW).await?;
    let Paired {
        uuid,
        key,
        manifest,
    } = client.paired().clone();
    let interfaces: Interfaces = client.get().await?;
    let status = client.get::<LinkStatus>().await.ok();
    let tv = Tv {
        address: ip,
        uuid: Some(uuid),
        key,
        manifest,
        mac: interfaces.wake_address(status.as_ref()),
    };
    write_private(out, &serde_json::to_vec_pretty(&tv)?)?;
    println!("paired; written to {}", out.display());
    client.close().await;
    Ok(())
}

fn probe_tv() -> Result<Tv> {
    let path = std::env::var("WEBOS_PROBE_TV").map_err(|_| "set WEBOS_PROBE_TV")?;
    Ok(serde_json::from_slice(&fs::read(&path)?)?)
}

async fn connect(options: &Options) -> Result<Client> {
    let tv = probe_tv()?;
    let uuid = match tv.uuid {
        Some(uuid) => uuid,
        None => {
            webos::hello(Endpoint::secure(tv.address), options)
                .await?
                .uuid
        }
    };
    let paired = Paired {
        uuid,
        key: tv.key,
        manifest: tv.manifest,
    };
    let started = Instant::now();
    // A key issued over one port may not count on the other; the TV would then show a prompt.
    let client = if std::env::var_os("WEBOS_PROBE_PLAIN").is_some() {
        Client::connect(Endpoint::plain(tv.address), &paired, options).await?
    } else {
        Client::connect_to(tv.address, &paired, options).await?
    };
    eprintln!(
        "connected to {} in {:.2} s",
        client.endpoint().url(),
        started.elapsed().as_secs_f64()
    );
    Ok(client)
}

/// Read-only requests whose answers become the captured frames that the synthetic test data in
/// `tests/data` is checked against, under the same file names.
const CAPTURES: [(&str, &str, &str); 12] = [
    ("system-info.json", "system/getSystemInfo", "{}"),
    (
        "power-state.json",
        "com.webos.service.tvpower/power/getPowerState",
        "{}",
    ),
    (
        "foreground-app.json",
        "com.webos.applicationManager/getForegroundAppInfo",
        "{}",
    ),
    ("volume.json", "audio/getVolume", "{}"),
    (
        "sound-output.json",
        "com.webos.service.apiadapter/audio/getSoundOutput",
        "{}",
    ),
    (
        "sound-settings.json",
        "settings/getSystemSettings",
        r#"{"category":"sound","keys":["soundMode","soundOutput"]}"#,
    ),
    (
        "launch-points.json",
        "com.webos.applicationManager/listLaunchPoints",
        "{}",
    ),
    ("inputs.json", "tv/getExternalInputList", "{}"),
    (
        "connection-info.json",
        "com.webos.service.connectionmanager/getinfo",
        "{}",
    ),
    (
        "pointer-socket.json",
        "com.webos.service.networkinput/getPointerInputSocket",
        "{}",
    ),
    // Refused on webOS 6 whatever the pairing grants.
    ("error-permission.json", "system.launcher/getAppState", "{}"),
    (
        "error-settings-key.json",
        "settings/getSystemSettings",
        r#"{"category":"picture","keys":["pictureMode"]}"#,
    ),
];

async fn capture(client: &Client, out: &Path) -> Result {
    DirBuilder::new().recursive(true).mode(0o700).create(out)?;
    write_private(
        &out.join("hello.json"),
        &serde_json::to_vec_pretty(&client.hello().raw)?,
    )?;
    println!("hello.json");
    for (name, uri, payload) in CAPTURES {
        let frame = client.frame(uri, serde_json::from_str(payload)?).await?;
        write_private(&out.join(name), &serde_json::to_vec_pretty(&frame)?)?;
        println!("{name}  {}", frame["type"].as_str().unwrap_or("?"));
    }
    Ok(())
}

async fn request(client: &Client, uri: &str, payload: Value) -> Result {
    let started = Instant::now();
    let frame = client.frame(uri, payload).await?;
    println!("{}", serde_json::to_string_pretty(&frame)?);
    eprintln!("answered in {:.2} s", started.elapsed().as_secs_f64());
    Ok(())
}

async fn watch(client: &Client, seconds: u64, uris: &[&str]) -> Result {
    let mut subscriptions = Vec::new();
    for uri in uris {
        let subscription = client.subscribe(uri, json!({})).await?;
        println!("{uri}  {}", subscription.current());
        subscriptions.push(subscription);
    }
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let changes = async {
        loop {
            let next = subscriptions.iter_mut().map(|s| {
                Box::pin(async {
                    let change = s.next().await;
                    (s.uri().to_owned(), change)
                })
            });
            let ((uri, change), _, _) = futures_util::future::select_all(next).await;
            let Some(change) = change else {
                println!("the connection closed");
                return;
            };
            println!(
                "{:7.2} s  {uri}  {}",
                started.elapsed().as_secs_f64(),
                change.map_or_else(|error| error.to_string(), |value| value.to_string())
            );
        }
    };
    let _ = tokio::time::timeout_at(deadline, changes).await;
    Ok(())
}

/// Writes a file only the user can read: these files hold keys and identifiers.
fn write_private(path: &Path, bytes: &[u8]) -> Result {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    Ok(())
}

/// The typed reports, without identifiers.
async fn state(client: &Client) -> Result {
    let power: PowerState = client.get().await?;
    println!(
        "power      {:?} on={} screen={}",
        power.mode,
        power.is_on(),
        power.is_screen_on()
    );
    let app: ForegroundApp = client.get().await?;
    println!("app        {}", app.0);
    let volume: Volume = client.get().await?;
    println!(
        "volume     {}/{} muted={} output={:?}",
        volume.level, volume.max, volume.muted, volume.output
    );
    let output: SoundOutput = client.get().await?;
    println!("output     {}", output.0);
    let picture: Picture = client.get().await?;
    println!("picture    {picture:?}");
    let wake: WakeOnLan = client.get().await?;
    let link: LinkStatus = client.get().await?;
    println!("wake       setting={} link={link:?}", wake.0);
    let apps: LaunchPoints = client.get().await?;
    println!("apps       {}", apps.0.len());
    let inputs: Inputs = client.get().await?;
    for input in inputs.0 {
        println!(
            "input      {} {:?} plugged={} connected={} {:?}",
            input.id, input.label, input.plugged, input.connected, input.devices
        );
    }
    Ok(())
}

async fn discover(seconds: u64) -> Result {
    for tv in webos::discover(Duration::from_secs(seconds)).await? {
        println!(
            "{}  model {:?}  firmware {:?}  {:?}",
            tv.name, tv.model, tv.firmware, tv.addresses
        );
    }
    Ok(())
}

/// Presses the buttons in order, half a second apart.
async fn keys(client: &Client, names: &[&str]) -> Result {
    let buttons: Vec<Button> = names
        .iter()
        .map(|name| Button::from_name(name).ok_or_else(|| format!("no button {name}")))
        .collect::<Result<_, _>>()?;
    let remote = client.remote().await?;
    for (i, button) in buttons.into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        remote.button(button).await?;
        println!("{}", button.name());
    }
    remote.close().await;
    Ok(())
}

/// Wakes the TV from the probe file and measures until its TLS port answers.
async fn wake() -> Result {
    let tv = probe_tv()?;
    let mac = tv.mac.ok_or("the probe file has no mac; pair again")?;
    let started = Instant::now();
    webos::wake(mac).await?;
    let options = Options::lg()?;
    while started.elapsed() < Duration::from_secs(30) {
        if webos::hello(Endpoint::secure(tv.address), &options)
            .await
            .is_ok()
        {
            println!(
                "the TV answers after {:.1} s",
                started.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("no answer within 30 s".into())
}
