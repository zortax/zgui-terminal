//! The component an application mounts.
//!
//! The grid the view draws is the screen and nothing more: one row element per row, each holding
//! one signal, none of them ever moving. Scrolling asks the emulator to put older lines on the
//! screen, and the rows are told what they now hold. Nothing here follows a scroll position, so
//! nothing here can disagree with one.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RenderEffect;

use crate::clipboard::{Clipboard, default_clipboard};
use crate::config::TerminalConfig;
use crate::core::command::TerminalCommand;
use crate::core::update::FrameUpdate;
use crate::event::{MouseShape, TerminalEvent};
use crate::grid::{Input, PointFilter, PointerPhase};
use crate::handle::TerminalHandle;
use crate::input::keyboard::{KeyBindings, KeyState, encode_key};
use crate::input::mouse::{MouseReport, encode as encode_mouse};
use crate::render::CellMetrics;
use crate::render::grid::{GridElement, GridLayout};
use crate::render::row::{RowElement, RowShared};
use crate::render::shaping::ShapingCache;
use crate::scroll::{ScrollRequest, ScrollState, WheelAction, lines_from_pixels, wheel_action};
use crate::search::SearchState;
use crate::selection::{ClickCounter, button_selects, selection_type, selects_rather_than_reports};
use crate::snapshot::{GridDimensions, OverlaySnapshot, RowSnapshot};
use crate::transport::{TerminalSize, TransportSource};

/// What is asked about a key before the terminal encodes it.
pub type KeyFilter = Box<dyn Fn(&zgui::vocab::KeyEvent, Modifiers) -> bool>;

/// How wide the bar showing where the screen sits in the history is, in device pixels.
const BAR_WIDTH: f32 = 8.0;

/// A terminal emulator over `transport`.
///
/// The transport is taken once, when the component is built. Everything the terminal reports is
/// offered twice: through the named callbacks for the common cases, and through `on_event` for
/// an application that wants one place to answer them.
///
/// ```no_run
/// use zgui::prelude::*;
/// use zgui_terminal::{Terminal, TerminalProps};
/// use zgui_terminal::transport::Channels;
///
/// # fn example(input: flume::Sender<Vec<u8>>, output: flume::Receiver<Vec<u8>>) -> impl IntoView {
/// view! {
///     Terminal(transport = Channels::new(input, output))
/// }
/// # }
/// ```
#[component]
#[allow(clippy::too_many_arguments)]
pub fn Terminal(
    /// What carries bytes to and from the program.
    ///
    /// The terminal it starts belongs to this view: unmounting stops the program. Hand a
    /// [`TerminalSession`](crate::TerminalSession) through `session` instead for one that should
    /// carry on.
    #[prop(optional, into)]
    transport: TransportSource,
    /// A terminal that is already running, and that outlives this view.
    #[prop(optional)]
    session: Option<crate::TerminalSession>,
    /// The colours, the measurements and the behaviour.
    #[prop(optional, into)]
    config: Option<TerminalConfig>,
    /// Where copied text goes. The desktop's clipboard by default.
    #[prop(optional)]
    clipboard: Option<Rc<dyn Clipboard>>,
    /// Called once with the handle, so the application can drive the terminal.
    #[prop(optional)]
    on_ready: Option<Box<dyn Fn(TerminalHandle)>>,
    /// Everything the terminal reports.
    #[prop(optional)]
    on_event: Option<Box<dyn Fn(TerminalEvent)>>,
    /// The title the program asked for.
    #[prop(optional)]
    on_title: Option<Box<dyn Fn(Option<String>)>>,
    /// The program ended.
    #[prop(optional)]
    on_exit: Option<Box<dyn Fn(crate::transport::ExitStatus)>>,
    /// A key before the terminal encodes it. Answer `true` to keep it from the program.
    #[prop(optional)]
    on_key: Option<KeyFilter>,
    /// Whether what a person types reaches the program.
    ///
    /// [`Input::Held`] is for an application answering the keyboard itself. Scrolling and
    /// selecting with the pointer carry on either way.
    #[prop(into, default = Signal::stored_local(Input::Live))]
    input: Signal<Input, LocalStorage>,
    /// A pointer before the terminal acts on it. Answer `true` to keep it from the terminal.
    #[prop(optional)]
    on_point: Option<PointFilter>,
    /// Classes the caller put on the terminal.
    #[prop(into, optional)]
    class: Classes,
    /// Anything else the caller forwarded.
    #[prop(attrs)]
    attrs: Attrs,
) -> impl IntoView {
    let config = config.unwrap_or_default();
    let clipboard = clipboard.unwrap_or_else(default_clipboard);
    let bindings = KeyBindings {
        alt_sends_escape: config.alt_sends_escape,
    };

    // What the text is drawn with. Measuring the face is what decides how large a cell is, and
    // everything else follows from that.
    let fonts = use_context::<zgui::app::Fonts>();
    let shaper = fonts.as_ref().map(|fonts| fonts.shaper());
    let families = shaper
        .as_ref()
        .map(|shaper| {
            crate::render::families::resolve(shaper, &config.font_family, config.font_size)
        })
        .unwrap_or_default();
    let metrics = measure_cell(shaper.as_ref(), &families, &config, 1.0, 0);
    let metrics_signal = RwSignal::new_local(metrics);

    let initial = TerminalSize {
        columns: 80,
        lines: 24,
        cell_width: metrics.width.min(u32::from(u16::MAX)) as u16,
        cell_height: metrics.height.min(u32::from(u16::MAX)) as u16,
    };
    // Either the caller's, which outlives this view, or one of this view's own, which does not.
    // The difference is only who holds a handle: a session with none left stops its program.
    let (session, borrowed) = match session {
        Some(session) => (session, true),
        None => {
            let started = transport
                .take()
                .map(|transport| crate::TerminalSession::start(transport, config.clone(), initial));
            match started {
                Some(Ok(session)) => (session, false),
                Some(Err(error)) => {
                    if let Some(report) = on_event.as_ref() {
                        report(TerminalEvent::Failed(error));
                    }
                    return AnyView::new(
                        view! { box(class = "terminal terminal--failed", class = class) },
                    );
                }
                None => {
                    return AnyView::new(
                        view! { box(class = "terminal terminal--failed", class = class) },
                    );
                }
            }
        }
    };

    let title = RwSignal::new_local(None::<String>);
    let dimensions = RwSignal::new_local(GridDimensions::default());
    let scroll_state = RwSignal::new_local(ScrollState::default());
    let search_state = RwSignal::new_local(SearchState::default());
    let size_signal = RwSignal::new_local(initial);
    let overlay = RwSignal::new_local(OverlaySnapshot::default());
    let mouse_shape = RwSignal::new_local(MouseShape::default());
    let hovered_link = RwSignal::new_local(None::<crate::event::Hyperlink>);
    let config_signal = RwSignal::new_local(config.clone());
    // How many rows the screen has, which is how many row elements there are.
    let screen_lines = RwSignal::new_local(0usize);
    // Counts the times the cell was measured again, so every row knows to draw itself afresh.
    let restyle = RwSignal::new_local(0u32);
    // Counts the frames that drew something, so an application reading the lines knows when to
    // read them again.
    let revision = RwSignal::new_local(0u64);

    // The grid's own element, and what a pointer position is measured against: its box in the
    // window's space, with every ancestor's origin summed in.
    let port = NodeRef::new();

    let handle = TerminalHandle {
        session: session.clone(),
        title,
        dimensions,
        scroll: scroll_state,
        search: search_state,
        metrics: metrics_signal,
        size: size_signal,
        config: config_signal,
        revision,
        port,
        overlay,
    };

    // One signal per row of the screen. A row keeps its element and its signal for as long as the
    // screen is that size; what changes is what the signal holds.
    let rows: Rc<RefCell<Vec<RwSignal<RowSnapshot, LocalStorage>>>> =
        Rc::new(RefCell::new(Vec::new()));

    let shared = Rc::new(RefCell::new(RowShared {
        metrics,
        palette: config.colors,
        columns: 80,
        builtin_glyphs: config.builtin_glyphs,
        blink_visible: true,
        shaping: ShapingCache::new(families.clone(), config.letter_spacing),
        shaper,
    }));
    let layout = Rc::new(RefCell::new(GridLayout {
        metrics,
        columns: 80,
        screen_lines: 0,
        padding: config.padding.max(0.0) as u32,
        background: config.colors.background,
        history: 0,
        display_offset: 0,
        thumb: config.colors.ansi[8],
    }));

    // What a style sheet said the colours are. The grid reads it, because that is where the
    // computed style is handed over, and reports it here.
    let styled_palette = RwSignal::new_local(None::<crate::config::ColorPalette>);
    let (grid_view, grid_handle) =
        zgui::custom::custom(GridElement::new(Rc::clone(&layout)).with_palette(
            config.colors,
            Rc::new(move |palette| styled_palette.set(Some(palette))),
        ));

    // What arrives from the thread that owns the emulator.
    let apply = {
        let rows = Rc::clone(&rows);
        let layout = Rc::clone(&layout);
        let shared = Rc::clone(&shared);
        let grid_handle = grid_handle.clone();
        let clipboard = Rc::clone(&clipboard);
        let owner = Owner::current();
        move |update: FrameUpdate| match update {
            FrameUpdate::Rows {
                rows: changed,
                dimensions: grid,
                scroll,
                rebuild,
            } => {
                let wanted = usize::from(grid.screen_lines);
                let resized = rows.borrow().len() != wanted;
                if resized {
                    // The rows grow and shrink; the ones that stay keep their signals. The keyed
                    // list keeps a row component wherever its key stays, and a kept component
                    // keeps watching the signal it was built on — a signal replaced here would
                    // leave it watching one nothing writes to again, which is a row that stays
                    // blank whatever the program does.
                    //
                    // The signals are made in the terminal's own scope, so nothing that unmounts
                    // a row disposes them.
                    let mut table = rows.borrow_mut();
                    if wanted < table.len() {
                        table.truncate(wanted);
                    } else {
                        let have = table.len();
                        let grow = |table: &mut Vec<RwSignal<RowSnapshot, LocalStorage>>| {
                            table
                                .extend((have..wanted).map(|row| {
                                    RwSignal::new_local(RowSnapshot::empty(row as u16))
                                }));
                        };
                        match owner.as_ref() {
                            Some(owner) => owner.with(|| grow(&mut table)),
                            None => grow(&mut table),
                        }
                    }
                }
                for row in changed {
                    if let Some(signal) = rows.borrow().get(usize::from(row.row)) {
                        signal.set(row);
                    }
                }

                {
                    let mut layout = layout.borrow_mut();
                    layout.columns = grid.columns;
                    layout.screen_lines = grid.screen_lines;
                    layout.history = grid.history;
                    layout.display_offset = grid.display_offset;
                }
                shared.borrow_mut().columns = grid.columns;
                dimensions.set(grid);
                scroll_state.set(scroll);
                if resized {
                    screen_lines.set(wanted);
                }
                let _ = rebuild;
                revision.set(revision.get_untracked().wrapping_add(1));
                grid_handle.relayout();
            }
            FrameUpdate::Overlay(snapshot) => overlay.set(*snapshot),
            FrameUpdate::Hyperlink(link) => hovered_link.set(link),
            FrameUpdate::Title(new_title) => title.set(new_title),
            FrameUpdate::Search(state) => search_state.set(state),
            FrameUpdate::MouseShape(shape) => mouse_shape.set(shape),
            FrameUpdate::ClipboardStore(kind, text) => clipboard.store(kind, text),
            FrameUpdate::ClipboardLoad { kind, reply } => clipboard.load(kind, reply),
            FrameUpdate::Selection(kind, Some(text)) => clipboard.store(kind, text),
            _ => {}
        }
    };

    // The reports an application asked to hear about.
    let report = {
        let on_event = on_event;
        let on_title = on_title;
        let on_exit = on_exit;
        move |update: &FrameUpdate| {
            let event = match update {
                FrameUpdate::Title(title) => {
                    if let Some(report) = on_title.as_ref() {
                        report(title.clone());
                    }
                    Some(TerminalEvent::Title(title.clone()))
                }
                FrameUpdate::Bell => Some(TerminalEvent::Bell),
                FrameUpdate::Exited(status) => {
                    if let Some(report) = on_exit.as_ref() {
                        report(*status);
                    }
                    Some(TerminalEvent::Exited(*status))
                }
                FrameUpdate::MouseShape(shape) => Some(TerminalEvent::MouseShape(*shape)),
                FrameUpdate::Failed(error) => Some(TerminalEvent::Failed(
                    crate::transport::TransportError::new(error.to_string()),
                )),
                FrameUpdate::CursorBlink(blinking) => Some(TerminalEvent::CursorBlink(*blinking)),
                _ => None,
            };
            if let (Some(event), Some(on_event)) = (event, on_event.as_ref()) {
                on_event(event);
            }
        }
    };

    let updates = session.watch();
    let pump = spawn_local({
        async move {
            while let Ok(update) = updates.recv_async().await {
                report(&update);
                apply(update);
            }
        }
    });
    on_cleanup_local({
        // A session the caller owns keeps running: this view only stops drawing it. One this
        // view started has no handle left anywhere and stops with it.
        let session = session.clone();
        move || {
            drop(pump);
            if borrowed {
                session.unwatch();
            }
        }
    });

    // The colours everything draws with: the configuration, with whatever a style sheet named
    // over it. What a program sets while it runs is put over both, where the rows are built.
    let colors = {
        let session = session.clone();
        let layout = Rc::clone(&layout);
        let shared = Rc::clone(&shared);
        let grid_handle = grid_handle.clone();
        let applied = Rc::new(RefCell::new(config.colors));
        RenderEffect::new(move |_| {
            let config = config_signal.get();
            let palette = styled_palette.get().unwrap_or(config.colors);
            if *applied.borrow() == palette {
                return;
            }
            *applied.borrow_mut() = palette;

            shared.borrow_mut().palette = palette;
            {
                let mut layout = layout.borrow_mut();
                layout.background = palette.background;
                layout.thumb = palette.ansi[8];
            }
            // The rows are built with the colours already resolved, so the thread that builds
            // them is told and every row is read out again.
            let mut with_colors = config.clone();
            with_colors.colors = palette;
            session.send(TerminalCommand::SetColors(Box::new(with_colors)));
            grid_handle.relayout();
        })
    };
    on_cleanup_local(move || drop(colors));

    // The size of the grid follows the size of the space it is given.
    //
    // The node's own box is *observed*, and the observation is made here rather than inside the
    // effect: one made inside is made again on every run, and a fresh one answers nothing, which
    // is an effect that spins and learns nothing. Made once, it is the only thing that reports a
    // grid resized by something other than the window — a pane dragged wider, a sidebar opened —
    // and without it the grid keeps its old size until the program next draws something.
    let port_box = port.observe_border_box();
    let window = try_use_window();
    let resize = {
        let session = session.clone();
        let layout = Rc::clone(&layout);
        let shared = Rc::clone(&shared);
        let grid_handle = grid_handle.clone();
        let measured = Rc::new(RefCell::new(config.clone()));
        let window = window.clone();
        RenderEffect::new(move |_| {
            let config = config_signal.get();
            if let Some(window) = window.as_ref() {
                let _ = window.size().get();
                let _ = window.scale().get();
            }
            let _ = dimensions.get();
            // Only while the node is actually there. The observation goes on holding the last box
            // it saw after the handle has gone, and `scale` answers one for a node that is not
            // bound — so carrying on here re-measures the face at the wrong scale and lays the
            // whole grid out again, every frame, for as long as that lasts. Reading the box
            // straight off the node used to stop this by answering nothing; observing it does not.
            if port.get_untracked().is_none() {
                return;
            }
            let scale = port.scale();
            // The observation before the one-shot read: the first answers nothing until the node
            // has been measured, and the box is wanted on that first frame too.
            let Some(bounds) = port_box.get().or_else(|| port.bounds()) else {
                return;
            };
            // A box of no size is not a grid of two columns by one line, which is what the metrics
            // would round it up to. It is a terminal that is not being laid out — hidden behind
            // another buffer, or part-way through a resize — and telling the program it is two
            // columns wide reflows everything it has drawn. It keeps the size it had until there
            // is a real one to give it.
            if bounds.size.width.0 <= 0.0 || bounds.size.height.0 <= 0.0 {
                return;
            }

            let mut metrics = metrics_signal.get_untracked();
            // The face is measured again when the scale changes, because every length here is a
            // whole device pixel, and when the configuration changes, because the size, the
            // family and the line height are what it was measured from.
            let restyled = *measured.borrow() != config;
            if restyled || (metrics.scale - scale).abs() > f32::EPSILON {
                let generation = metrics.generation.wrapping_add(1);
                let mut shared = shared.borrow_mut();
                let shaper = shared.shaper.take();
                let families = shaper
                    .as_ref()
                    .map(|shaper| {
                        crate::render::families::resolve(
                            shaper,
                            &config.font_family,
                            config.font_size,
                        )
                    })
                    .unwrap_or_default();
                metrics = measure_cell(shaper.as_ref(), &families, &config, scale, generation);
                shared.shaper = shaper;
                shared.metrics = metrics;
                shared.builtin_glyphs = config.builtin_glyphs;
                shared.shaping.set_families(families, config.letter_spacing);
                drop(shared);

                let mut layout = layout.borrow_mut();
                layout.metrics = metrics;
                layout.padding = (config.padding * scale).max(0.0).round() as u32;
                drop(layout);

                // The style sheet is read over whatever the configuration now says.
                grid_handle.update(|grid| grid.set_base(config.colors));

                *measured.borrow_mut() = config.clone();
                metrics_signal.set(metrics);
                restyle.update(|count| *count = count.wrapping_add(1));
                grid_handle.relayout();
            }

            let (columns, lines) = metrics.grid_size(
                bounds.size.width.0,
                bounds.size.height.0,
                config.padding * scale,
            );
            let size = TerminalSize {
                columns,
                lines,
                cell_width: metrics.width.min(u32::from(u16::MAX)) as u16,
                cell_height: metrics.height.min(u32::from(u16::MAX)) as u16,
            };
            if size != size_signal.get_untracked() {
                size_signal.set(size);
                session.send(TerminalCommand::Resize(size));
            }
        })
    };
    on_cleanup_local(move || drop(resize));

    // What a person does with the keyboard.
    let key_down = {
        let session = session.clone();
        let bindings = bindings.clone();
        let config = config.clone();
        let clipboard = Rc::clone(&clipboard);
        move |cx: &mut EventCx<'_, events::KeyDown>| {
            let event: zgui::vocab::KeyEvent = (*cx).clone();
            let modifiers = cx.modifiers;
            if let Some(on_key) = on_key.as_ref()
                && on_key(&event, modifiers)
            {
                cx.stop_propagation();
                cx.prevent_default();
                return;
            }
            // The application is reading the keyboard itself, so the program reads none of it.
            if !input.get_untracked().is_live() {
                cx.stop_propagation();
                return;
            }
            // Copy and paste, on the keys every terminal spares for them.
            match crate::input::clipboard_chord(&event, modifiers) {
                Some(crate::input::ClipboardChord::Copy) => {
                    session.send(TerminalCommand::CopySelection(ClipboardKind::Standard));
                    cx.stop_propagation();
                    cx.prevent_default();
                    return;
                }
                Some(crate::input::ClipboardChord::Paste) => {
                    let session = session.clone();
                    clipboard.read_text(
                        ClipboardKind::Standard,
                        Box::new(move |text| {
                            if let Some(text) = text {
                                session.send(TerminalCommand::Paste(text));
                            }
                        }),
                    );
                    cx.stop_propagation();
                    cx.prevent_default();
                    return;
                }
                None => {}
            }
            let mode = session.mode();
            if let Some(bytes) = encode_key(&event, modifiers, KeyState::Down, mode, &bindings) {
                session.send(TerminalCommand::Write(bytes.to_vec()));
                if config.scroll_to_bottom_on_input {
                    session.send(TerminalCommand::Scroll(ScrollRequest::Bottom));
                }
                // A key the program reads is the program's alone: tab moves no focus and enter
                // activates nothing.
                cx.stop_propagation();
                cx.prevent_default();
            }
        }
    };

    let key_up = {
        let session = session.clone();
        let bindings = bindings.clone();
        move |cx: &mut EventCx<'_, events::KeyUp>| {
            // A release is held back with its press. Under the kitty protocol a program is told
            // about both, so letting one through reports a key nobody pressed.
            if !input.get_untracked().is_live() {
                return;
            }
            let mode = session.mode();
            if let Some(bytes) = encode_key(&*cx, cx.modifiers, KeyState::Up, mode, &bindings) {
                session.send(TerminalCommand::Write(bytes.to_vec()));
            }
        }
    };

    // What a person composed through an input method.
    let ime_commit = {
        let session = session.clone();
        move |cx: &mut EventCx<'_, events::ImeCommit>| {
            if !input.get_untracked().is_live() {
                return;
            }
            if let zgui::vocab::ImeEvent::Commit(text) = &**cx
                && !text.is_empty()
            {
                session.send(TerminalCommand::Write(text.as_bytes().to_vec()));
            }
        }
    };

    // What a person does with the pointer.
    let clicks = Rc::new(RefCell::new(ClickCounter::new()));
    let on_point = on_point.map(Rc::new);
    // Whether the application took the gesture with its pointer filter.
    let pointing = Rc::new(std::cell::Cell::new(false));
    let dragging = Rc::new(std::cell::Cell::new(false));
    let dragging_bar = Rc::new(std::cell::Cell::new(false));
    let wheel_remainder = Rc::new(std::cell::Cell::new(0.0f32));

    let pointer_down = {
        let session = session.clone();
        let layout = Rc::clone(&layout);
        let clipboard = Rc::clone(&clipboard);
        let clicks = Rc::clone(&clicks);
        let dragging = Rc::clone(&dragging);
        let dragging_bar = Rc::clone(&dragging_bar);
        let pointing = Rc::clone(&pointing);
        let on_point = on_point.clone();
        move |cx: &mut EventCx<'_, events::PointerDown>| {
            let Some(bounds) = port.window_bounds() else {
                return;
            };
            let scale = port.scale();
            let inside = (
                cx.position.x.0 * scale - bounds.origin.x.0,
                cx.position.y.0 * scale - bounds.origin.y.0,
            );

            // The bar showing where the screen sits in the history, when there is one.
            if layout.borrow().has_history() && inside.0 >= bounds.size.width.0 - BAR_WIDTH {
                let offset = layout.borrow().offset_at(inside.1, bounds.size.height.0);
                session.send(TerminalCommand::ScrollTo(offset));
                dragging_bar.set(true);
                cx.capture_pointer();
                cx.request_focus(cx.current);
                return;
            }

            let Some(point) = grid_point(inside, &layout) else {
                return;
            };
            // The application owns the gesture when it says so. The pointer is captured for it,
            // so a drag that leaves the terminal is still reported here.
            if let Some(filter) = on_point.as_ref()
                && filter(point.point(&layout), PointerPhase::Down)
            {
                pointing.set(true);
                cx.capture_pointer();
                cx.request_focus(cx.current);
                return;
            }
            let mode = session.mode();
            let modifiers = cx.modifiers;

            if selects_rather_than_reports(modifiers, mode) {
                // The middle button pastes the primary selection, as X has always had it.
                if cx.button == Some(PointerButton::Middle) {
                    let session = session.clone();
                    clipboard.read_text(
                        ClipboardKind::Primary,
                        Box::new(move |text| {
                            if let Some(text) = text {
                                session.send(TerminalCommand::Paste(text));
                            }
                        }),
                    );
                    cx.request_focus(cx.current);
                    return;
                }
                if !button_selects(cx.button) {
                    cx.request_focus(cx.current);
                    return;
                }
                let count = clicks.borrow_mut().click_at(
                    point.column,
                    point.row,
                    std::time::Instant::now(),
                );
                session.send(TerminalCommand::SelectionStart {
                    point: point.grid,
                    side: point.side,
                    kind: selection_type(count, modifiers),
                });
                dragging.set(true);
                // The pointer belongs to the terminal until it comes up, so a selection dragged
                // past the edge of the window keeps being reported here.
                cx.capture_pointer();
            } else if input.get_untracked().is_live()
                && let Some(bytes) = encode_mouse(
                    MouseReport::Press(cx.button.unwrap_or_default()),
                    point.column,
                    point.row,
                    modifiers,
                    mode,
                )
            {
                session.send(TerminalCommand::Write(bytes.to_vec()));
            }
            cx.request_focus(cx.current);
        }
    };

    let pointer_move = {
        let session = session.clone();
        let layout = Rc::clone(&layout);
        let dragging = Rc::clone(&dragging);
        let dragging_bar = Rc::clone(&dragging_bar);
        let pointing = Rc::clone(&pointing);
        let on_point = on_point.clone();
        move |cx: &mut EventCx<'_, events::PointerMove>| {
            let Some(bounds) = port.window_bounds() else {
                return;
            };
            let scale = port.scale();
            let inside = (
                cx.position.x.0 * scale - bounds.origin.x.0,
                cx.position.y.0 * scale - bounds.origin.y.0,
            );

            if dragging_bar.get() {
                let offset = layout.borrow().offset_at(inside.1, bounds.size.height.0);
                session.send(TerminalCommand::ScrollTo(offset));
                return;
            }

            let Some(point) = grid_point(inside, &layout) else {
                return;
            };
            if pointing.get() {
                if let Some(filter) = on_point.as_ref() {
                    filter(point.point(&layout), PointerPhase::Move);
                }
                return;
            }
            if dragging.get() {
                session.send(TerminalCommand::SelectionUpdate {
                    point: point.grid,
                    side: point.side,
                });
                return;
            }
            let mode = session.mode();
            session.send(TerminalCommand::HyperlinkAt(Some(point.grid)));
            if input.get_untracked().is_live()
                && let Some(bytes) = encode_mouse(
                    MouseReport::Motion(cx.button),
                    point.column,
                    point.row,
                    cx.modifiers,
                    mode,
                )
            {
                session.send(TerminalCommand::Write(bytes.to_vec()));
            }
        }
    };

    let pointer_up = {
        let session = session.clone();
        let layout = Rc::clone(&layout);
        let dragging = Rc::clone(&dragging);
        let dragging_bar = Rc::clone(&dragging_bar);
        let pointing = Rc::clone(&pointing);
        let on_point = on_point.clone();
        let config = config.clone();
        move |cx: &mut EventCx<'_, events::PointerUp>| {
            let was_dragging = dragging.replace(false);
            let was_bar = dragging_bar.replace(false);
            let was_pointing = pointing.replace(false);
            if was_dragging || was_bar || was_pointing {
                cx.release_pointer();
            }
            if was_bar {
                return;
            }
            let Some(bounds) = port.window_bounds() else {
                return;
            };
            let scale = port.scale();
            let inside = (
                cx.position.x.0 * scale - bounds.origin.x.0,
                cx.position.y.0 * scale - bounds.origin.y.0,
            );
            let Some(point) = grid_point(inside, &layout) else {
                return;
            };
            if was_pointing {
                if let Some(filter) = on_point.as_ref() {
                    filter(point.point(&layout), PointerPhase::Up);
                }
                return;
            }
            if was_dragging {
                if config.copy_on_select {
                    session.send(TerminalCommand::CopySelection(ClipboardKind::Primary));
                }
                return;
            }
            let mode = session.mode();
            if input.get_untracked().is_live()
                && let Some(bytes) = encode_mouse(
                    MouseReport::Release(cx.button.unwrap_or_default()),
                    point.column,
                    point.row,
                    cx.modifiers,
                    mode,
                )
            {
                session.send(TerminalCommand::Write(bytes.to_vec()));
            }
        }
    };

    let wheel = {
        let session = session.clone();
        let layout = Rc::clone(&layout);
        let remainder = Rc::clone(&wheel_remainder);
        let config = config.clone();
        move |cx: &mut EventCx<'_, events::Wheel>| {
            // Which cell the pointer is over. A program with more than one pane scrolls the one
            // under the pointer, and it can only know which that is from the report.
            let at = port.window_bounds().and_then(|bounds| {
                let scale = port.scale();
                let inside = (
                    cx.position.x.0 * scale - bounds.origin.x.0,
                    cx.position.y.0 * scale - bounds.origin.y.0,
                );
                grid_point(inside, &layout)
            });
            let cell_height = layout.borrow().metrics.height as f32;
            let lines = match cx.delta {
                ScrollDelta::Lines { y, .. } => (y * f32::from(config.scroll_multiplier)) as i32,
                ScrollDelta::Pixels(delta) => {
                    let mut left = remainder.get();
                    let lines = lines_from_pixels(delta.height.0, cell_height, &mut left);
                    remainder.set(left);
                    lines
                }
                _ => return,
            };
            if lines == 0 {
                return;
            }
            // A program that grabbed the wheel reads none of it while the keyboard is held; the
            // view scrolls instead, which is what a mode reading the history wants.
            let mode = if input.get_untracked().is_live() {
                session.mode()
            } else {
                alacritty_terminal::term::TermMode::empty()
            };
            // Positive moves the view toward the newest line, which is scrolling down.
            match wheel_action(-lines, cx.modifiers.shift(), mode) {
                WheelAction::Scroll(lines) => {
                    session.send(TerminalCommand::Scroll(ScrollRequest::Lines(lines)));
                }
                WheelAction::Report(notches) => {
                    let report = if notches > 0 {
                        MouseReport::WheelUp
                    } else {
                        MouseReport::WheelDown
                    };
                    let (column, row) = at
                        .as_ref()
                        .map(|point| (point.column, point.row))
                        .unwrap_or((0, 0));
                    for _ in 0..notches.abs() {
                        if let Some(bytes) = encode_mouse(report, column, row, cx.modifiers, mode) {
                            session.send(TerminalCommand::Write(bytes.to_vec()));
                        }
                    }
                }
                WheelAction::Arrows(notches) => {
                    let arrow: &[u8] = if notches > 0 { b"\x1bOA" } else { b"\x1bOB" };
                    let plain: &[u8] = if notches > 0 { b"\x1b[A" } else { b"\x1b[B" };
                    let sequence = if mode.contains(alacritty_terminal::term::TermMode::APP_CURSOR)
                    {
                        arrow
                    } else {
                        plain
                    };
                    let mut bytes = Vec::new();
                    for _ in 0..notches.abs().min(10) {
                        bytes.extend_from_slice(sequence);
                    }
                    session.send(TerminalCommand::Write(bytes));
                }
            }
            cx.stop_propagation();
        }
    };

    // Nothing is repainted here. The emulator is told, and what it publishes over the rows says
    // whether the terminal has focus — so the rows that draw a cursor draw it again, and the rest
    // do nothing at all.
    let focus_in = {
        let session = session.clone();
        move |_: &mut EventCx<'_, events::FocusIn>| {
            session.send(TerminalCommand::SetFocus(true));
        }
    };
    let focus_out = {
        let session = session.clone();
        move |_: &mut EventCx<'_, events::FocusOut>| {
            session.send(TerminalCommand::SetFocus(false));
        }
    };

    // The terminal takes the keys as soon as it is there.
    let claim_focus = set_timeout(std::time::Duration::ZERO, move || port.focus());
    on_cleanup_local(move || drop(claim_focus));

    if let Some(ready) = on_ready.as_ref() {
        ready(handle.clone());
    }
    provide_local_context(handle);

    // Each row is its own element, so a row that did not change is drawn from what it drew last
    // time and a row that did costs only itself.
    let build_row = {
        let rows = Rc::clone(&rows);
        let shared = Rc::clone(&shared);
        move |row: usize| {
            let Some(signal) = rows.borrow().get(row).copied() else {
                return AnyView::new(view! { box() });
            };
            let (element, handle) = zgui::custom::custom(RowElement::new(
                signal.get_untracked(),
                overlay.get_untracked().for_row(row as u16),
                Rc::clone(&shared),
            ));
            let follow = RenderEffect::new({
                let handle = handle.clone();
                let drawn_for = std::cell::Cell::new(restyle.get_untracked());
                move |_| {
                    let snapshot = signal.get();
                    let over = overlay.get().for_row(row as u16);
                    let generation = restyle.get();
                    handle.update(|element| {
                        element.set_row(snapshot);
                        element.set_overlay(over);
                    });
                    // A cell that changed size moves the row as well as what is in it, so the row
                    // is laid out again rather than only drawn again.
                    if generation == drawn_for.get() {
                        handle.repaint();
                    } else {
                        drawn_for.set(generation);
                        handle.relayout();
                    }
                }
            });
            on_cleanup_local(move || drop(follow));
            AnyView::new(element.class("terminal__row"))
        }
    };

    let grid = grid_view.class("terminal__grid").child(view! {
        for row in move || (0..screen_lines.get()).collect::<Vec<_>>(), key = |row: &usize| *row {
            {build_row(row)}
        }
    });

    AnyView::new(view! {
        box(
            class = "terminal",
            class = class,
            node_ref = port,
            tabindex = Focus::Sequential,
            on:key_down = key_down,
            on:key_up = key_up,
            on:ime_commit = ime_commit,
            on:pointer_down = pointer_down,
            on:pointer_move = pointer_move,
            on:pointer_up = pointer_up,
            on:wheel = wheel,
            on:focus_in = focus_in,
            on:focus_out = focus_out,
            {..attrs}
        ) {
            {grid.into_view()}
        }
    })
}

/// Where a pointer event falls in the grid.
struct Hit {
    /// Which column, counting from zero.
    column: usize,
    /// Which row of the screen.
    row: usize,
    /// Which half of the cell the pointer is in.
    side: alacritty_terminal::index::Side,
    /// The same place, as the emulator counts it.
    grid: alacritty_terminal::index::Point,
}

impl Hit {
    /// The same place, counted from the oldest line the history holds.
    fn point(&self, layout: &Rc<RefCell<GridLayout>>) -> crate::grid::GridPoint {
        let history = layout.borrow().history as i64;
        crate::grid::GridPoint {
            line: (history + i64::from(self.grid.line.0)).max(0) as usize,
            column: self.column.min(usize::from(u16::MAX)) as u16,
        }
    }
}

/// Where a pointer `inside` the grid, in device pixels from its corner, falls.
///
/// The grid is the screen, so a row of one is a row of the other and nothing has to be mapped
/// through a scroll position.
fn grid_point(inside: (f32, f32), layout: &Rc<RefCell<GridLayout>>) -> Option<Hit> {
    let layout = layout.borrow();
    let metrics = layout.metrics;
    if metrics.width == 0 || metrics.height == 0 || layout.screen_lines == 0 {
        return None;
    }
    let padding = layout.padding as f32;

    let column_f = (inside.0 - padding) / metrics.width as f32;
    let column = (column_f.floor().max(0.0) as usize).min(usize::from(layout.columns) - 1);
    let right_half = column_f - column as f32 >= 0.5;

    let row_f = (inside.1 - padding) / metrics.height as f32;
    let row = (row_f.floor().max(0.0) as usize).min(usize::from(layout.screen_lines) - 1);

    // The emulator counts from the top of the screen it writes to, which sits this far above the
    // newest line.
    let index = i32::try_from(row as i64 - layout.display_offset as i64).ok()?;
    let side = if right_half {
        alacritty_terminal::index::Side::Right
    } else {
        alacritty_terminal::index::Side::Left
    };
    Some(Hit {
        column,
        row,
        side,
        grid: alacritty_terminal::index::Point::new(
            alacritty_terminal::index::Line(index),
            alacritty_terminal::index::Column(column),
        ),
    })
}

/// How large a cell is for `config`, at `scale`.
///
/// `families` are already resolved to faces this machine has, so the measurements and the glyphs
/// come from the same face.
fn measure_cell(
    shaper: Option<&zgui::app::Shaper>,
    families: &[zgui_interned::Ident],
    config: &TerminalConfig,
    scale: f32,
    generation: u32,
) -> CellMetrics {
    let size = config.font_size * scale;
    let fallback = CellMetrics {
        generation,
        ..CellMetrics::default()
    };
    let (Some(shaper), false) = (shaper, families.is_empty()) else {
        return fallback;
    };
    let mut request = crate::render::families::request(families, size);
    request.letter_spacing = config.letter_spacing * scale;
    match shaper.line_metrics(&request) {
        Some(metrics) => CellMetrics::from_face(
            &metrics,
            size,
            scale,
            config.line_height,
            config.letter_spacing,
            generation,
        ),
        None => fallback,
    }
}
