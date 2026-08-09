# zgui-terminal

An embeddable terminal emulator for [Zgui](https://github.com/zortax/zgui) applications.

The component draws a grid of cells and talks to a program through a transport the application
supplies. Nothing in it names a process or a file descriptor, so the same component serves a
local shell, a kubernetes pod, an SSH session or a test script.

```rust
use zgui::prelude::*;
use zgui_terminal::{Terminal, TerminalProps};
use zgui_terminal::transport::Pty;
use portable_pty::CommandBuilder;

#[component]
fn Shell() -> impl IntoView {
    let transport = Pty::spawn(CommandBuilder::new("/bin/sh")).expect("a shell");
    view! { Terminal(transport = transport) }
}
```

## What it does

- **Any source of bytes.** [`Transport`] is one small trait. `Pty` runs a program on this
  machine, `Channels` reaches a peer that speaks in channels, and `ReadWrite` wraps a pair of
  byte streams. A transport with a runtime of its own implements the trait in about fifteen
  lines, and no runtime type appears in the contract.
- **Pixel-perfect box drawing.** Box drawing, block elements, powerline separators and the
  rounded caps beside them are built for the cell the grid measured rather than taken from the
  font, so they join with no seam at any size or scale.
- **A row at a time.** Each row is a retained element. A row that did not change is drawn from
  what it drew last time, so a moving cursor costs two rows and a full-screen program costs the
  rows it rewrote.
- **The whole keyboard.** The ordinary sequences and the kitty protocol, including the reports
  that tell a press from a release and control from a control character.
- **Everything a terminal is expected to have.** Scrollback, selection by character, word, line
  and rectangle, mouse reporting, bracketed paste, hyperlinks, a search through the history,
  and the clipboard.

## How it is put together

```text
  Transport thread ──bytes──┐
  Term events ──────────────┼──► pump thread ──updates──► UI thread
  UI commands ──────────────┘    (vte parsing,            (row signals,
                                  damage to rows)          painting)
                                       │
                                       └──replies──► writer thread ──► Transport
```

Escape sequences are parsed off the interface thread, and only the rows that changed are
published. Writes to the program go on a queue, so a program that has stopped reading never
blocks the window.

## Running the examples

```bash
cargo run --example local_shell --features pty --release   # a shell on this machine
cargo run --example echo_stream --release                  # a peer made of channels
cargo run --example pattern --release                      # the built-in glyphs, drawn large
```

## Colours

The colours are the style sheet's. The terminal's own `color` is the text; everything else is a
custom property, so a theme is written where the rest of an application's theme is and answers to
the same selectors.

```css
.terminal {
    color: #cdd6f4;
    --terminal-background: #1e1e2e;
    --terminal-red: #f38ba8;
    --terminal-bright-red: #f38ba8;
    --terminal-cursor: #f5e0dc;
    --terminal-selection: #414458;
}
```

The sixteen named colours are `--terminal-black` through `--terminal-white` and
`--terminal-bright-black` through `--terminal-bright-white`. The rest are
`--terminal-background`, `--terminal-foreground`, `--terminal-cursor`, `--terminal-cursor-text`,
`--terminal-selection`, `--terminal-selection-text`, `--terminal-match` and
`--terminal-match-current`.

Three things decide a colour, in this order:

1. [`TerminalConfig::colors`], which is what an application with no style sheet gets.
2. A custom property, one entry at a time, so naming one colour leaves the rest alone.
3. What the program sets while it runs, through `OSC 4` and its neighbours. A program that sets a
   colour has said what it wants.

## Features

| Feature | What it adds |
|---|---|
| `pty` | `Pty`, a transport that runs a program behind a local pseudo terminal |

## Performance

`zgui-terminal` is fast and can benefit from `zgui`'s incremental rendering model. It generally
performs in line with other GPU accelerated terminal emulators like Alacritty or Ghostty, while
sometimes outperforming them on sparse output changes. See the [benchmarks](benchmarks/) for
`vtebench` results.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
