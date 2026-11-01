# telka

Terminal remote for LG webOS TVs over their local API: an interactive TUI that works like the
remote control, and one-shot commands for the shell.

Unofficial project, not affiliated with LG Electronics. It talks to the TV's undocumented local
SSAP API and acts on a real TV that someone may be watching.

## Status

Early development: the workspace and CI are set up, the `telka` CLI does not do anything yet.
Reading the TV's state over the local API has been verified live on an LG C1 (OLED65C11LB) with
webOS 6.5.3.

## Pairing

`telka pair` finds the TV on the network and asks you to accept telka on the TV with the remote.
The key the TV issues is kept locally, readable only by you, together with the TV's identity.
Nothing leaves your home network. To turn the TV on, telka sends Wake-on-LAN, which needs
"Turn on via Wi-Fi" in the TV's settings.

## Using the API responsibly

- Pair once and keep the key private: it lets anyone on your network control the TV.
- Every command acts on a real TV. Tests never reach a real TV, and live checks change one thing
  at a time and put it back.
- Never log or commit the key, serial numbers, MAC addresses or captured TV responses.

## Development

| Crate | Purpose |
|---|---|
| `crates/webos` | LG webOS SSAP client, no UI dependencies |
| `crates/telka` | the `telka` binary: CLI and TUI |

Test data in `crates/webos/tests/data` is synthetic: one SSAP frame per file, written by hand.
Never commit captured TV responses, keys or real device identifiers. Code, docs and commit
messages are in English.

The checks CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --locked
cargo deny check
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
