//! A terminal running a shell on this machine.
//!
//! Run it with `cargo run --example local_shell --features pty --release`.
//!
//! What it is worth reading for:
//!
//! * the component knows nothing about processes. It is handed a [`Pty`], which is one
//!   implementation of the transport seam; the echo example hands it a pair of channels instead
//!   and the component is the same;
//! * the size of the grid follows the size of the window, and the program is told about it,
//!   because the terminal works the columns and lines out from the cell it measured;
//! * the title a program sets becomes the window's own title, and the moment it ends arrives as
//!   an ordinary callback;
//! * the colours are the style sheet's. The terminal's own `color` is the text, and every other
//!   colour is a custom property, so a theme is written where the rest of an application's theme
//!   is and answers to the same selectors.

use portable_pty::CommandBuilder;
use zgui::prelude::*;
use zgui::vocab::{Key, KeyEvent};
use zgui_terminal::transport::Pty;
use zgui_terminal::{KeyFilter, TerminalConfig, TerminalHandle, TerminalProps};

/// A window holding one terminal.
#[component]
fn Shell() -> impl IntoView {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut command = CommandBuilder::new(&shell);
    // What a program looks up to learn what this terminal can do. The disambiguated keyboard
    // reports are asked for by the program at run time rather than through this name, so the
    // ordinary name is the one that works on a machine at the far end of a connection too.
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    if let Ok(directory) = std::env::current_dir() {
        command.cwd(directory);
    }

    let transport = match Pty::spawn(command) {
        Ok(transport) => transport,
        Err(error) => {
            eprintln!("cannot start {shell}: {error}");
            std::process::exit(1);
        }
    };

    let config = TerminalConfig {
        font_family: vec![
            "Mononoki Nerd Font Mono".to_owned(),
            "Mononoki Nerd Font".to_owned(),
            "DejaVu Sans Mono".to_owned(),
        ],
        font_size: DEFAULT_FONT_SIZE,
        line_height: 1.2,
        // The space around the cells. It sits inside the terminal rather than around it, so the
        // scroll bar stays at the window's edge with the spacing to the left of it.
        padding: 10.0,
        scrollback: 10_000,
        copy_on_select: true,
        ..TerminalConfig::default()
    };

    let windows = use_windows();
    let window = use_window();

    // The handle the terminal hands over when it is built, which is what the size shortcuts
    // change the configuration through.
    let terminal = RwSignal::new_local(None::<TerminalHandle>);

    // Control and plus, minus or zero, as every terminal has them. These are answered before the
    // terminal encodes the key, so the program behind it never sees them.
    let on_key = move |event: &KeyEvent, modifiers: Modifiers| {
        if !modifiers.control() {
            return false;
        }
        let Key::Character(text) = &event.key_without_modifiers else {
            return false;
        };
        let step = match text.as_str() {
            "+" | "=" => 1.0,
            "-" => -1.0,
            "0" => 0.0,
            _ => return false,
        };
        let Some(handle) = terminal.get_untracked() else {
            return false;
        };
        handle.update_config(|config| {
            config.font_size = if step == 0.0 {
                DEFAULT_FONT_SIZE
            } else {
                // Bounded either way: a cell of no pixels draws nothing, and one the size of the
                // window leaves no grid to draw.
                (config.font_size + step).clamp(6.0, 72.0)
            };
        });
        true
    };

    // The title a program sets is the window's own, which is where a desktop shows it.
    let on_title = move |title: Option<String>| {
        window.set_title(title.unwrap_or_else(|| "zgui-terminal".to_owned()));
    };

    view! {
        column(class = "shell") {
            Terminal(
                class = "shell__terminal",
                transport = transport,
                config = config,
                on_ready = Box::new(move |handle| terminal.set(Some(handle))) as Box<dyn Fn(_)>,
                on_key = Box::new(on_key) as KeyFilter,
                on_title = Box::new(on_title) as Box<dyn Fn(_)>,
                on_exit = Box::new(move |_| windows.quit()) as Box<dyn Fn(_)>,
            )
        }
    }
}

/// The size the terminal starts at, and the one control and zero returns it to.
const DEFAULT_FONT_SIZE: f32 = 14.0;

/// How it looks. The terminal fills what it is given and draws its own colours.
const SHEET: &str = css!(
    ":root {
        background-color: #1e1e2e;
        color: #cdd6f4;
        font-family: sans-serif;
    }

    .shell {
        width: 100%;
        height: 100%;
    }

    .shell__terminal {
        flex: 1;
        min-height: 0;

        /* The text, which is the element's own colour, and the background behind the cells. */
        color: #cdd6f4;
        --terminal-background: #1e1e2e;

        /* The eight a program addresses by name, and the eight bright ones after them. */
        --terminal-black: #45475a;
        --terminal-red: #f38ba8;
        --terminal-green: #a6e3a1;
        --terminal-yellow: #f9e2af;
        --terminal-blue: #89b4fa;
        --terminal-magenta: #f5c2e7;
        --terminal-cyan: #94e2d5;
        --terminal-white: #bac2de;
        --terminal-bright-black: #585b70;
        --terminal-bright-red: #f38ba8;
        --terminal-bright-green: #a6e3a1;
        --terminal-bright-yellow: #f9e2af;
        --terminal-bright-blue: #89b4fa;
        --terminal-bright-magenta: #f5c2e7;
        --terminal-bright-cyan: #94e2d5;
        --terminal-bright-white: #a6adc8;

        --terminal-cursor: #f5e0dc;
        --terminal-cursor-text: #1e1e2e;
        --terminal-selection: #414458;
    }"
);

fn main() -> Result<(), zgui::Error> {
    app()
        .with_application_id("dev.zgui.Terminal")
        .with_title("zgui-terminal")
        // No title bar of its own: the program behind the terminal names the window, and a
        // desktop shows that name where it shows every other window's.
        .with_decorations(Decorations::NoTitleBar)
        .with_size(900.0, 560.0)
        .with_stylesheet(SHEET)
        .run(|| view! { Shell() })
}
