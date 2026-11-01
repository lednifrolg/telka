# telka

Terminal remote for LG webOS TVs over their local API: an interactive TUI that works like the
remote control, and one-shot commands for the shell.

Unofficial project, not affiliated with LG Electronics. It talks to the TV's undocumented local
SSAP API and acts on a real TV that someone may be watching.

![The shell with telka status and telka vol, then the TUI pressing the d-pad, turning the volume and the OLED light, setting a picture mode, opening the web browser and typing an address into it, a volume change from the remote, and the help](assets/demo.gif)

## Status

First release: the command line and the interactive TUI are complete, with typing into the TV,
the mouse as its pointer and the picture settings. What telka asks of the TV was verified live
on an LG C1 (OLED65C11LB) with webOS 6.5.3: pairing, buttons, volume, apps and inputs, the
screen, typing, toasts, picture settings and Wake-on-LAN. telka itself is tested against a fake
TV, and was tried on the C1 only where it changes nothing.

## Install

Download the archive for your system from the
[latest release](https://github.com/lednifrolg/telka/releases/latest) and put `telka` on your
PATH:

```sh
# Linux: x86_64-unknown-linux-musl or aarch64-unknown-linux-musl
# macOS: aarch64-apple-darwin (Apple silicon) or x86_64-apple-darwin (Intel)
target=x86_64-unknown-linux-musl
latest=https://github.com/lednifrolg/telka/releases/latest/download
curl -fsSLO "$latest/telka-$target.tar.gz"
tar -xzf "telka-$target.tar.gz"
mkdir -p ~/.local/bin && install -m 755 "telka-$target/telka" ~/.local/bin/
```

The archive also holds shell completions and man pages. With the GitHub CLI, this checks that
the archive was built by this repository's release workflow:

```sh
gh attestation verify "telka-$target.tar.gz" --repo lednifrolg/telka \
  --signer-workflow lednifrolg/telka/.github/workflows/release.yml
```

From source, with a recent Rust:

```sh
cargo install --git https://github.com/lednifrolg/telka --tag v0.1.0 --locked telka
```

or from a clone, `cargo install --path crates/telka --locked`. The binary writes its shell
completions and man pages too:

```sh
telka completions bash > ~/.local/share/bash-completion/completions/telka
telka completions zsh > ~/.zfunc/_telka      # a directory on your $fpath
telka completions fish > ~/.config/fish/completions/telka.fish
telka man ~/.local/share/man/man1
```

## Usage

```sh
telka pair                  # finds the TV; accept the prompt on the TV with the remote
telka                       # the interactive TUI
telka status                # power, what is in front, volume, sound and picture (--json)

telka on | off | toggle     # off by the power button, on over Wi-Fi (Wake-on-LAN)
telka screen off | on       # the panel alone; the sound keeps playing
telka vol 15                # or relative: +2, -2
telka mute | unmute
telka app youtube           # by title or id; --url opens a video or a page
telka input ps5             # by port, label or the device on it
telka key home down ok      # remote buttons in order; --list shows them all
telka type "breaking bad" --enter   # into the TV's text field with focus
telka play | pause | stop
telka notify "Laundry is done"
telka picture --light 70 --energy auto --mode cinema   # without options, shows them
telka watch                 # every change as it happens, from the remote too (--json)

telka apps | inputs | discover | unpair
```

Names match regardless of case and diacritics, and part of a name is enough when it is
unambiguous. A command waits until the TV reports the change, at most a few seconds;
`--no-wait` returns once the TV took it. `telka <command> --help` has the details.

The TUI is a remote: arrows, Enter and Backspace go to the TV as its buttons, and letters are
telka's own actions. It shows what the TV does, live, and every change in the activity pane, from
telka or from anywhere else; a change telka sent shows at once and gets its ✓ when the TV
reports it. A TV that is off is looked for every few seconds, and `p` wakes it. The TUI pairs
with the TV on its first start.

| Key | Does |
|---|---|
| `←↑↓→` `⏎` `⌫` | the remote's arrows, OK and back (`Esc` closes what is open first) |
| `h` `s` | home, the TV's settings |
| `␣` `,` `.` | pause and play in turn, rewind, fast forward |
| `+` `-` `m` | volume, mute |
| `a` `i` | pick an app or an input: type to filter, `⏎` to open |
| `k` | any other button of the remote by name: info, guide, colors, numbers, … |
| `t` | type into the TV: letters go out as you type, `⏎` is Enter on the TV |
| `[` `]` `e` | OLED light by 5, energy saving to the next level |
| `v` | pick the picture mode |
| `d` | screen off or on; the sound stays |
| `p` | wake the TV; when it is on, twice to turn it off |
| `R` `?` `q` | read everything again, every key, quit |

The line for typing also opens on its own when a text field on the TV gets focus; only fields
with the TV's own keyboard take it. In the remote pane, a click on a button presses it, and
elsewhere the mouse is the TV's pointer: a drag moves it, a click clicks, the wheel scrolls. The
wheel over the volume turns it. In a narrow window, `Tab` switches between the TV and the
activity.

Picture settings go through an alert that the TV runs and closes at once, unseen; telka then
reads them back. When the alert stays on the TV, telka presses OK, which closes it and makes the
change, and marks the change with ⚠. The TV does not tell its picture mode, so the TUI shows the
one telka set.

What the TV allows, learned on the C1: `telka on` wakes it only after it went off by its power
button, which `telka off` presses, and it takes up to a minute; a toast does not show over an
app on the full screen; apps with their own keyboard, like YouTube, ignore typed text; picture
settings need the signed manifest, and the TV does not tell its picture mode.

## Pairing

`telka pair` finds the TV on the network, or takes its address, and asks you to accept telka on
the TV with the remote; the prompt names LG's remote app, whose signed manifest telka uses. The
key the TV issues is kept in `~/.local/state/telka/tv.json`, readable only by you, together with
the TV's identity: another TV at the address is refused. When the TV's address changes, telka
finds it again by its serial number. Nothing leaves your home network. To turn the TV on, telka
sends Wake-on-LAN, which needs "Turn on via Wi-Fi" in the TV's settings.

## Configuration

`~/.config/telka/config.toml` is optional:

```toml
tv = "192.168.1.30"   # a fixed address: telka never looks for the TV on the network
pointer_speed = 12    # pixels on the TV per column a drag moves the pointer
animations = false    # no lit buttons, flashing changes or running bars in the TUI
```

`TELKA_HOME` puts config and state in one directory.

## Recipes

Keys for the TV in Hyprland's Lua config. Each press is a new connection to the TV, so leave the
bindings without `repeating`; `--no-wait` keeps them quick:

```lua
hl.bind("SUPER + F11", hl.dsp.exec_cmd("telka vol -2 --no-wait"))
hl.bind("SUPER + F12", hl.dsp.exec_cmd("telka vol +2 --no-wait"))
hl.bind("SUPER + F10", hl.dsp.exec_cmd("telka toggle"))
hl.bind("SUPER + SHIFT + F10", hl.dsp.exec_cmd("telka screen off"))
```

With `hyprland.conf` instead, `bind = SUPER, F12, exec, telka vol +2 --no-wait`; in Omarchy,
`o.bind("SUPER + F12", "TV louder", "telka vol +2 --no-wait")` in `~/.config/hypr/bindings.lua`.

A toast on the TV when something is done, from the shell or from a hook, like
`telka notify "System updated"` in an Omarchy hook at
`~/.config/omarchy/hooks/post-update.d/telka`:

```sh
cargo build --release; telka notify "cargo build: exit $?"
```

`telka watch --json` tells every change as a JSON line, from the remote too, and keeps looking
for a TV that went off. Pause the music on the computer when the TV comes on:

```sh
telka watch --json |
  jq --unbuffered -r 'select(.what == "power" and .from == "off" and .to == "on") | .time' |
  while read -r _; do playerctl pause; done
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success; also `telka status` for a TV that is off |
| 1 | the TV does not answer or failed, or a local file problem |
| 2 | wrong usage, or something the pairing does not allow |
| 4 | not paired, the TV no longer accepts the key, or another TV answers |
| 5 | sent, but the TV did not report the change in time |

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
Tests run against fake TVs on the loopback interface: the CLI tests run the binary against one
that `TELKA_TV_URL` points at, and the TUI tests run it in a pseudo-terminal, press keys and read
the screen. Never commit captured TV responses, keys or real device identifiers. Code, docs and
commit messages are in English.

`cargo run -p webos --example probe` talks to a real TV for live checks: `hello`, `pair`,
`capture` (read-only frames for comparing the synthetic data, kept out of the repository),
`request` and `watch`. The fake TV of the tests, with a few more apps, also serves demos and
screenshots, and [`assets/demo.tape`](assets/demo.tape) records the GIF above with
[vhs](https://github.com/charmbracelet/vhs):

```sh
export TELKA_HOME=$(mktemp -d) TELKA_TV_URL=ws://127.0.0.1:18893
cargo run -p telka --example demo_tv -- 18893 "$TELKA_HOME" &
cargo run -p telka

cargo build -p telka --bin telka --examples && vhs assets/demo.tape
```

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
