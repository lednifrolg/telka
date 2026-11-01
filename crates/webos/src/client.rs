//! A registered connection to a TV: requests and subscriptions over one WebSocket.
//!
//! A task owns the WebSocket. It sends what the [`Client`] handles ask for, hands every answer to
//! whoever waits for its `id`, and pings the TV; when the TV stops answering or closes, every
//! waiting request fails with [`Error::Closed`] and every subscription ends.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior, interval_at, timeout};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::error::Error;
use crate::frame::{self, Incoming};
use crate::pair::{ClientKey, Hello, Manifest, Options, Timing, greet, register_with_key};
use crate::transport::{Endpoint, Socket};

/// What a client needs to connect again after pairing. Keep it private: the key lets anyone on
/// the network control the TV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paired {
    /// The TV's `deviceUUID`; connecting fails with [`Error::WrongTv`] when another TV answers.
    pub uuid: String,
    pub key: ClientKey,
    pub manifest: Manifest,
}

/// A connection to a TV that accepted the client key. Clones share the connection, which stays
/// open while a clone or a [`Subscription`] lives.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    commands: mpsc::UnboundedSender<Command>,
    next_id: AtomicU64,
    endpoint: Endpoint,
    hello: Hello,
    paired: Paired,
    options: Options,
    closed: watch::Receiver<bool>,
}

enum Command {
    Send {
        id: String,
        uri: String,
        frame: String,
        waiter: Waiter,
    },
    /// The caller stopped waiting.
    Forget {
        id: String,
    },
    Unsubscribe {
        id: String,
        uri: String,
    },
    Close,
}

/// Who waits for the answers to one `id`.
enum Waiter {
    /// The payload of a request, or why the TV refused.
    Request(oneshot::Sender<Result<Value, Error>>),
    /// The whole answer frame, whatever its type.
    Frame(oneshot::Sender<Result<Value, Error>>),
    /// The current state, then every change.
    Subscription {
        first: Option<oneshot::Sender<Result<Value, Error>>>,
        updates: mpsc::UnboundedSender<Result<Value, Error>>,
    },
}

impl Waiter {
    fn fail(self, error: Error) {
        match self {
            Self::Request(reply) | Self::Frame(reply) => {
                let _ = reply.send(Err(error));
            }
            Self::Subscription { first, .. } => {
                if let Some(first) = first {
                    let _ = first.send(Err(error));
                }
            }
        }
    }
}

struct Pending {
    uri: String,
    waiter: Waiter,
}

impl Client {
    /// Connects to the TV at `endpoint` with the key from pairing.
    pub async fn connect(
        endpoint: Endpoint,
        paired: &Paired,
        options: &Options,
    ) -> Result<Self, Error> {
        let (mut socket, hello) = greet(endpoint, options).await?;
        let registered = if hello.uuid == paired.uuid {
            register_with_key(&mut socket, paired.manifest, &paired.key, &options.timing).await
        } else {
            Err(Error::WrongTv {
                expected: paired.uuid.clone(),
                found: hello.uuid.clone(),
            })
        };
        let key = match registered {
            Ok(key) => key,
            Err(error) => {
                // Closing also takes a pairing prompt for an unknown key off the screen.
                let _ = timeout(options.timing.connect, socket.close(None)).await;
                return Err(error);
            }
        };
        Ok(Self::start(
            socket,
            endpoint,
            hello,
            key,
            paired.manifest,
            options,
        ))
    }

    /// Connects over TLS, or over the plain port when the TLS port does not answer. A certificate
    /// that is not LG's, another TV or a refused key is never retried in plain.
    pub async fn connect_to(ip: IpAddr, paired: &Paired, options: &Options) -> Result<Self, Error> {
        match Self::connect(Endpoint::secure(ip), paired, options).await {
            Err(Error::Connect(_) | Error::TimedOut | Error::WebSocket(_)) => {
                Self::connect(Endpoint::plain(ip), paired, options).await
            }
            other => other,
        }
    }

    /// Runs the connection on `socket`, after the TV accepted `key`.
    pub(crate) fn start(
        socket: Socket,
        endpoint: Endpoint,
        hello: Hello,
        key: ClientKey,
        manifest: Manifest,
        options: &Options,
    ) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (closed_sender, closed) = watch::channel(false);
        tokio::spawn(run(socket, receiver, options.timing, closed_sender));
        let paired = Paired {
            uuid: hello.uuid.clone(),
            key,
            manifest,
        };
        Self {
            inner: Arc::new(Inner {
                commands,
                next_id: AtomicU64::new(1),
                endpoint,
                hello,
                paired,
                options: options.clone(),
                closed,
            }),
        }
    }

    pub fn hello(&self) -> &Hello {
        &self.inner.hello
    }

    pub fn endpoint(&self) -> Endpoint {
        self.inner.endpoint
    }

    pub(crate) fn options(&self) -> &Options {
        &self.inner.options
    }

    /// The TV's identity and the key it accepted: what to store for the next connection.
    pub fn paired(&self) -> &Paired {
        &self.inner.paired
    }

    /// Sends a request and waits for its answer. `uri` is like `audio/getVolume`, with or without
    /// the `ssap://` scheme.
    pub async fn request(&self, uri: &str, payload: Value) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        let id = self.send(uri, &payload, false, Waiter::Request(reply))?;
        self.wait(id, answer).await
    }

    /// Like [`Client::request`], reading the answer as `T`.
    pub async fn request_as<T: DeserializeOwned>(
        &self,
        uri: &str,
        payload: Value,
    ) -> Result<T, Error> {
        let answer = self.request(uri, payload).await?;
        serde_json::from_value(answer).map_err(|source| Error::Decode {
            uri: frame::uri(uri),
            source,
        })
    }

    /// Sends a request and returns the whole answer frame, also when it is an error: for
    /// diagnostics and for capturing what the TV sends.
    pub async fn frame(&self, uri: &str, payload: Value) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        let id = self.send(uri, &payload, false, Waiter::Frame(reply))?;
        self.wait(id, answer).await
    }

    /// Subscribes to `uri`: the TV answers with the current state, then again on every change.
    pub async fn subscribe(&self, uri: &str, payload: Value) -> Result<Subscription, Error> {
        let (reply, answer) = oneshot::channel();
        let (updates, receiver) = mpsc::unbounded_channel();
        let waiter = Waiter::Subscription {
            first: Some(reply),
            updates,
        };
        let id = self.send(uri, &payload, true, waiter)?;
        let current = self.wait(id.clone(), answer).await?;
        Ok(Subscription {
            id,
            uri: frame::uri(uri),
            current,
            updates: receiver,
            commands: self.inner.commands.clone(),
        })
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        *self.inner.closed.borrow()
    }

    /// Waits until the connection ends: the TV closed it or went silent.
    pub async fn closed(&self) {
        let mut closed = self.inner.closed.clone();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    /// Closes the connection for every clone and subscription.
    pub async fn close(&self) {
        let _ = self.inner.commands.send(Command::Close);
        self.closed().await;
    }

    fn send(
        &self,
        uri: &str,
        payload: &Value,
        subscribe: bool,
        waiter: Waiter,
    ) -> Result<String, Error> {
        let id = self
            .inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        let uri = frame::uri(uri);
        let frame = if subscribe {
            frame::subscribe(&id, &uri, payload)
        } else {
            frame::request(&id, &uri, payload)
        };
        self.inner
            .commands
            .send(Command::Send {
                id: id.clone(),
                uri,
                frame,
                waiter,
            })
            .map_err(|_| Error::Closed)?;
        Ok(id)
    }

    async fn wait(
        &self,
        id: String,
        answer: oneshot::Receiver<Result<Value, Error>>,
    ) -> Result<Value, Error> {
        match timeout(self.inner.options.timing.request, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => {
                let _ = self.inner.commands.send(Command::Forget { id });
                Err(Error::TimedOut)
            }
        }
    }
}

/// Changes the TV reports for one subscription. Dropping it unsubscribes.
pub struct Subscription {
    id: String,
    uri: String,
    current: Value,
    updates: mpsc::UnboundedReceiver<Result<Value, Error>>,
    commands: mpsc::UnboundedSender<Command>,
}

impl Subscription {
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// The state when the subscription started.
    pub fn current(&self) -> &Value {
        &self.current
    }

    /// The next change; `None` once the connection ended.
    pub async fn next(&mut self) -> Option<Result<Value, Error>> {
        self.updates.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Unsubscribe {
            id: self.id.clone(),
            uri: self.uri.clone(),
        });
    }
}

/// Owns the WebSocket until the TV closes it, goes silent, or every handle is gone.
async fn run(
    mut socket: Socket,
    mut commands: mpsc::UnboundedReceiver<Command>,
    timing: Timing,
    closed: watch::Sender<bool>,
) {
    let mut pending: HashMap<String, Pending> = HashMap::new();
    let mut ping = interval_at(Instant::now() + timing.ping_every, timing.ping_every);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Close) => {
                    let _ = timeout(timing.connect, socket.close(None)).await;
                    break;
                }
                Some(Command::Send { id, uri, frame, waiter }) => {
                    if let Err(error) = socket.send(Message::text(frame)).await {
                        waiter.fail(Error::WebSocket(error));
                        break;
                    }
                    pending.insert(id, Pending { uri, waiter });
                }
                Some(Command::Forget { id }) => {
                    pending.remove(&id);
                }
                Some(Command::Unsubscribe { id, uri }) => {
                    if pending.remove(&id).is_some()
                        && socket.send(Message::text(frame::unsubscribe(&id, &uri))).await.is_err()
                    {
                        break;
                    }
                }
            },
            message = socket.next() => {
                heard = Instant::now();
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Some((id, uri)) = route(&mut pending, text.as_str())
                            && socket.send(Message::text(frame::unsubscribe(&id, &uri))).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            },
            _ = ping.tick() => {
                if heard.elapsed() >= timing.silence
                    || socket.send(Message::Ping(Bytes::new())).await.is_err()
                {
                    break;
                }
            },
        }
    }
    for (_, pending) in pending.drain() {
        pending.waiter.fail(Error::Closed);
    }
    let _ = closed.send(true);
}

/// Hands a frame to whoever waits for its `id`. Returns a subscription nobody listens to any
/// more, to unsubscribe.
fn route(pending: &mut HashMap<String, Pending>, text: &str) -> Option<(String, String)> {
    let frame = Incoming::parse(text).ok()?;
    let id = frame.id.clone()?;
    let entry = pending.get_mut(&id)?;
    if let Waiter::Subscription { first, updates } = &mut entry.waiter {
        let outcome = frame.outcome(&entry.uri);
        if let Some(first) = first.take() {
            let failed = outcome.is_err();
            let _ = first.send(outcome);
            if failed {
                pending.remove(&id);
            }
        } else if updates.send(outcome).is_err() {
            let entry = pending.remove(&id)?;
            return Some((id, entry.uri));
        }
        return None;
    }
    let entry = pending.remove(&id)?;
    match entry.waiter {
        Waiter::Request(reply) => {
            let _ = reply.send(frame.outcome(&entry.uri));
        }
        Waiter::Frame(reply) => {
            let _ = reply.send(Ok(frame.raw));
        }
        Waiter::Subscription { .. } => unreachable!("handled above"),
    }
    None
}
