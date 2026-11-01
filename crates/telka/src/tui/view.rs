//! Drawing the TUI from the model.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use webos::Button;

use super::keymap::{Action, BINDINGS, Hint};
use super::model::{
    Dialog, Entry, Model, Origin, Overlay, Phase, Picker, PickerKind, Status, Tab, TextLine,
};
use crate::theme;

const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// Borders of panes without focus, and status line segments.
const MUTED: Color = Color::Indexed(8);
const GOOD: Color = Color::Indexed(2);
const WARN: Color = Color::Indexed(3);
const BAD: Color = Color::Indexed(1);
const ACCENT: Color = Color::Indexed(4);
const PRESS: Color = Color::Indexed(5);
const REMOTE: Color = Color::Indexed(3);
/// Background of the selected row.
const SELECTED: Color = Color::Indexed(8);
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Smallest window for the narrow layout, and for the wide one.
const TABS_MIN: (u16, u16) = (46, 16);
const WIDE_MIN: (u16, u16) = (100, 24);
/// Width of the TV pane in the wide layout, and height of the top row.
const TV_WIDTH: u16 = 46;
const TOP_HEIGHT: u16 = 10;
/// Width of the labels in the TV pane, and of the values after a bar.
const LABEL: usize = 9;
const VALUE: usize = 5;
/// Width of the "what" column in the activity.
const WHAT: usize = 7;

/// What the mouse can point at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A button in the remote pane: a click does what its key does.
    Act(Action),
    Tab(Tab),
    /// The volume bar: the wheel turns it.
    Volume,
    /// The remote pane around its buttons: the mouse there is the TV's pointer.
    Pad,
}

/// A screen area the mouse can point at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotspot {
    pub area: Rect,
    pub target: Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Wide,
    Tabs,
    TooSmall,
}

fn shape(area: Rect) -> Shape {
    let fits = |(width, height): (u16, u16)| area.width >= width && area.height >= height;
    if fits(WIDE_MIN) {
        Shape::Wide
    } else if fits(TABS_MIN) {
        Shape::Tabs
    } else {
        Shape::TooSmall
    }
}

pub fn render(model: &Model, frame: &mut Frame) -> Vec<Hotspot> {
    let area = frame.area();
    let shape = shape(area);
    let mut spots = Vec::new();
    if shape == Shape::TooSmall {
        too_small(frame, area);
        return spots;
    }
    let [body, status, hints] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    if shape == Shape::Wide {
        wide(model, frame, body, &mut spots);
    } else {
        tabs(model, frame, body, &mut spots);
    }
    statusline(model, frame, status);
    hint_bar(model, frame, hints, shape);
    match &model.phase {
        Phase::Pairing(dialog) => {
            pairing(model, dialog, frame, body);
            spots.clear();
        }
        Phase::Connecting if model.tv.is_none() => {
            let text = vec![Line::from(vec![
                spinner(model),
                Span::raw(" connecting to the TV"),
            ])];
            centered(frame, body, " telka ", text);
            spots.clear();
        }
        _ => {}
    }
    match &model.overlay {
        Overlay::Picker(picker) => {
            picker_dialog(model, picker, frame, area);
            spots.clear();
        }
        Overlay::Help => {
            help(frame, area);
            spots.clear();
        }
        Overlay::Text(line) => {
            text_line(line, frame, area);
            spots.clear();
        }
        Overlay::None => {}
    }
    spots
}

fn too_small(frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::from("window too small"),
        Line::styled(format!("{} × {} needed", TABS_MIN.0, TABS_MIN.1), DIM),
    ];
    let height = u16::try_from(text.len()).unwrap_or(u16::MAX);
    let middle = area.centered_vertically(Constraint::Length(height));
    frame.render_widget(Paragraph::new(text).centered(), middle);
}

fn pane(title: &str, focused: bool) -> Block<'_> {
    let color = if focused { ACCENT } else { MUTED };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Span::styled(
            format!(" {title} "),
            BOLD.fg(if focused { ACCENT } else { Color::Reset }),
        ))
        .padding(Padding::horizontal(1))
}

fn right_title(text: &str) -> Line<'static> {
    Line::styled(format!(" {text} "), DIM).right_aligned()
}

fn is_live(model: &Model) -> bool {
    model.phase == Phase::Live
}

fn tv_title(model: &Model) -> String {
    model
        .tv
        .as_ref()
        .map(|tv| tv.model.clone())
        .or_else(|| model.known.as_ref().map(|(model, _)| model.clone()))
        .unwrap_or_else(|| "TV".to_owned())
}

fn power_word(model: &Model) -> &'static str {
    match (&model.phase, &model.tv) {
        (Phase::Live, Some(tv)) => theme::power(&tv.power),
        (Phase::Waking { .. }, _) => "waking",
        (Phase::Connecting, _) => "connecting",
        (Phase::Pairing(_), _) => "pairing",
        _ => "off",
    }
}

fn wide(model: &Model, frame: &mut Frame, body: Rect, spots: &mut Vec<Hotspot>) {
    let [top, activity_area] =
        Layout::vertical([Constraint::Length(TOP_HEIGHT), Constraint::Fill(1)]).areas(body);
    let [tv_area, remote_area] =
        Layout::horizontal([Constraint::Length(TV_WIDTH), Constraint::Fill(1)]).areas(top);

    let title = tv_title(model);
    let block = pane(&title, false).title_top(right_title(power_word(model)));
    let inner = block.inner(tv_area);
    frame.render_widget(block, tv_area);
    tv(model, frame, inner, spots);

    let block = pane("remote", true).title_top(right_title("keys go to the TV"));
    let inner = block.inner(remote_area);
    frame.render_widget(block, remote_area);
    remote(model, frame, inner, spots);
    if !is_live(model) {
        frame.buffer_mut().set_style(inner, DIM);
    }

    let state = if is_live(model) { "live" } else { "off" };
    let block = pane("activity", false).title_top(right_title(state));
    let inner = block.inner(activity_area);
    frame.render_widget(block, activity_area);
    frame.render_widget(
        Paragraph::new(activity(model, usize::from(inner.width))),
        inner,
    );
}

fn tabs(model: &Model, frame: &mut Frame, body: Rect, spots: &mut Vec<Hotspot>) {
    let title = tv_title(model);
    let block = pane(&title, true).title_top(right_title(power_word(model)));
    let inner = block.inner(body);
    frame.render_widget(block, body);
    let [bar, content] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
    let mut x = bar.x;
    let mut spans = Vec::new();
    for (index, tab) in Tab::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", DIM));
            x += 5;
        }
        let label = format!("{} {}", index + 1, tab.name());
        let width = u16::try_from(label.chars().count()).unwrap_or(0);
        spots.push(Hotspot {
            area: Rect::new(x, bar.y, width, 1),
            target: Target::Tab(tab),
        });
        x += width;
        spans.push(if tab == model.tab {
            Span::styled(label, BOLD.fg(ACCENT).add_modifier(Modifier::UNDERLINED))
        } else {
            Span::styled(label, DIM)
        });
    }
    frame.render_widget(Line::from(spans), bar);
    match model.tab {
        Tab::Tv => {
            let [state, sent] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(content);
            tv(model, frame, state, spots);
            frame.render_widget(sent_line(model), sent);
        }
        Tab::Activity => {
            frame.render_widget(
                Paragraph::new(activity(model, usize::from(content.width))),
                content,
            );
        }
    }
}

fn label(text: &str) -> Span<'static> {
    Span::styled(fit(text, LABEL), DIM)
}

fn pill(text: &str, background: Color) -> Span<'static> {
    // Black reads poorly on the grey of a muted pill.
    let ink = if background == MUTED {
        Color::Indexed(15)
    } else {
        Color::Black
    };
    Span::styled(format!(" {text} "), BOLD.fg(ink).bg(background))
}

/// The TV pane: what the TV does, or why there is nothing to show.
fn tv(model: &Model, frame: &mut Frame, area: Rect, spots: &mut Vec<Hotspot>) {
    let width = usize::from(area.width);
    let address = model
        .tv
        .as_ref()
        .map(|tv| tv.address)
        .or(model.known.as_ref().map(|(_, address)| *address));
    let lines = match &model.phase {
        Phase::Off { reason, .. } => {
            let mut lines = vec![
                Line::from(vec![
                    pill("OFF", MUTED),
                    Span::styled("  the TV is off or asleep", DIM),
                ]),
                Line::default(),
            ];
            if let Some(reason) = reason {
                lines.push(Line::styled(fit(reason, width), Style::new().fg(BAD)));
            } else if let Some(address) = address {
                lines.push(Line::styled(format!("nothing answers at {address}"), DIM));
            }
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::styled("p", BOLD),
                Span::raw(" wakes it over Wi-Fi (Wake-on-LAN)"),
            ]));
            lines
        }
        Phase::Waking { since } => {
            let seconds = model.now.duration_since(*since).as_secs().max(0);
            vec![
                Line::from(vec![
                    pill("WAKE", WARN),
                    Span::styled("  a magic packet every second", DIM),
                ]),
                Line::default(),
                Line::from(vec![
                    spinner(model),
                    Span::styled(format!(" waiting for the TV to answer · {seconds} s"), DIM),
                ]),
                Line::styled("it takes up to a minute", DIM),
            ]
        }
        _ => live_lines(model, area, width, spots),
    };
    frame.render_widget(Paragraph::new(lines), area);
}

fn live_lines(
    model: &Model,
    area: Rect,
    width: usize,
    spots: &mut Vec<Hotspot>,
) -> Vec<Line<'static>> {
    let Some(tv) = &model.tv else {
        return Vec::new();
    };
    let screen_off = tv.power.mode == webos::PowerMode::ScreenOff;
    let head = if screen_off {
        pill("SCREEN OFF", WARN)
    } else {
        pill("ON", GOOD)
    };
    let title = fit(&model.in_front(), width.saturating_sub(6));
    let mut lines = vec![
        Line::from(vec![
            head,
            Span::raw("  "),
            Span::styled(title.trim_end().to_owned(), BOLD),
        ]),
        if screen_off {
            Line::styled("the panel is dark, the sound keeps playing", DIM)
        } else {
            Line::default()
        },
    ];
    let bar_width = width.saturating_sub(LABEL + VALUE);
    if let Some((level, muted)) = model.shown_volume() {
        let max = tv.volume.as_ref().map_or(100, |volume| volume.max);
        let value = if muted {
            Span::styled(format!("{:>VALUE$}", "muted"), BOLD.fg(WARN))
        } else {
            Span::raw(format!("{level:>VALUE$}"))
        };
        let shown = model.volume_bar.map_or(level, |bar| bar.at(model.now));
        let mut spans = vec![label("volume")];
        spans.extend(bar(shown, max, bar_width, !muted));
        spans.push(value);
        spots.push(Hotspot {
            area: Rect::new(area.x, area.y + 2, area.width, 1),
            target: Target::Volume,
        });
        lines.push(Line::from(spans));
    }
    let mut sound = Vec::new();
    sound.extend(tv.output.as_deref().map(theme::sound_output));
    sound.extend(tv.sound_mode.clone());
    lines.push(Line::from(vec![
        label("sound"),
        Span::raw(sound.join(" · ")),
    ]));
    lines.push(Line::default());
    if tv.picture.is_some() {
        if let Some(light) = model.shown_light() {
            let shown = model.light_bar.map_or(light, |bar| bar.at(model.now));
            let mut spans = vec![label("light")];
            spans.extend(bar(shown, 100, bar_width, true));
            spans.push(Span::raw(format!("{light:>VALUE$}")));
            lines.push(Line::from(spans));
        }
        if let Some(energy) = model.shown_energy() {
            lines.push(Line::from(vec![label("energy"), Span::raw(energy)]));
        }
        let mode = match (&model.picture_mode, tv.signed) {
            (Some(mode), _) => vec![
                Span::raw(mode.clone()),
                Span::styled(" · set by telka", DIM),
            ],
            (None, true) => vec![Span::styled("unknown · v sets it", DIM)],
            (None, false) => vec![Span::styled("the TV does not tell", DIM)],
        };
        let mut spans = vec![label("mode")];
        spans.extend(mode);
        lines.push(Line::from(spans));
    }
    lines
}

fn bar(value: u8, max: u8, width: usize, lit: bool) -> Vec<Span<'static>> {
    let filled = (usize::from(value) * width)
        .checked_div(usize::from(max.max(1)))
        .unwrap_or(0)
        .min(width);
    let color = if lit { ACCENT } else { MUTED };
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(color)),
        Span::styled("─".repeat(width - filled), Style::new().fg(MUTED)),
    ]
}

/// The remote pane: the buttons the keys stand for, each clickable, what the pointer did, and
/// what went out last.
fn remote(model: &Model, frame: &mut Frame, area: Rect, spots: &mut Vec<Hotspot>) {
    // The whole pane first: buttons drawn later win the click.
    spots.push(Hotspot {
        area,
        target: Target::Pad,
    });
    // (column, row, text, action); columns are in characters from the pane's left edge.
    let buttons: [(u16, u16, &str, Action); 15] = [
        (12, 1, "▲", Action::Press(Button::UP)),
        (8, 2, "◀", Action::Press(Button::LEFT)),
        (11, 2, "OK", Action::Press(Button::ENTER)),
        (15, 2, "▶", Action::Press(Button::RIGHT)),
        (12, 3, "▼", Action::Press(Button::DOWN)),
        (28, 1, "home", Action::Press(Button::HOME)),
        (37, 1, "settings", Action::Press(Button::MENU)),
        (28, 2, "back", Action::Press(Button::BACK)),
        (37, 2, "screen", Action::Screen),
        (28, 3, "apps", Action::Apps),
        (37, 3, "inputs", Action::Inputs),
        (5, 5, "◀◀", Action::Press(Button::REWIND)),
        (10, 5, "play/pause", Action::PlayPause),
        (23, 5, "▶▶", Action::Press(Button::FAST_FORWARD)),
        (37, 5, "mute", Action::Mute),
    ];
    let height = 8.min(area.height);
    let mut rows: Vec<Vec<(u16, Span<'static>)>> = vec![Vec::new(); usize::from(height)];
    for (column, row, text, action) in buttons {
        if row >= height || column + text_width(text) > area.width {
            continue;
        }
        rows[usize::from(row)].push((column, key_cap(model, text, action)));
        spots.push(Hotspot {
            area: Rect::new(area.x + column, area.y + row, text_width(text), 1),
            target: Target::Act(action),
        });
    }
    // The volume buttons, with "vol" between them.
    if 5 < height && 28 + 7 <= area.width {
        rows[5].push((28, key_cap(model, "-", Action::VolumeDown)));
        rows[5].push((30, Span::styled("vol", DIM)));
        rows[5].push((34, key_cap(model, "+", Action::VolumeUp)));
        for (column, action) in [(28, Action::VolumeDown), (34, Action::VolumeUp)] {
            spots.push(Hotspot {
                area: Rect::new(area.x + column, area.y + 5, 1, 1),
                target: Target::Act(action),
            });
        }
    }
    let mut lines: Vec<Line<'static>> = rows
        .into_iter()
        .map(|mut row| {
            row.sort_by_key(|(column, _)| *column);
            let mut spans = Vec::new();
            let mut at = 0;
            for (column, span) in row {
                spans.push(Span::raw(
                    " ".repeat(usize::from(column.saturating_sub(at))),
                ));
                at = column + u16::try_from(span.width()).unwrap_or(0);
                spans.push(span);
            }
            Line::from(spans)
        })
        .collect();
    if lines.len() > 6 {
        let pointer = model.pointer.as_ref().map_or_else(
            || Span::styled("drag here to move it · click · wheel scrolls", DIM),
            |pointer| Span::raw(pointer.clone()),
        );
        lines[6] = Line::from(vec![Span::styled(fit("pointer", LABEL), DIM), pointer]);
    }
    if lines.len() > 7 {
        lines[7] = sent_line(model);
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// A button of the remote pane, lit for a moment after it was pressed.
fn key_cap(model: &Model, text: &str, action: Action) -> Span<'static> {
    if model.lit(action) {
        Span::styled(text.to_owned(), BOLD.fg(Color::Black).bg(ACCENT))
    } else {
        Span::raw(text.to_owned())
    }
}

fn text_width(text: &str) -> u16 {
    u16::try_from(text.chars().count()).unwrap_or(u16::MAX)
}

fn sent_line(model: &Model) -> Line<'static> {
    let sent: Vec<&str> = model.sent.iter().copied().collect();
    let text = if sent.is_empty() {
        Span::styled("nothing yet", DIM)
    } else {
        Span::raw(sent.join(" "))
    };
    Line::from(vec![Span::styled(fit("sent", LABEL), DIM), text])
}

fn activity(model: &Model, width: usize) -> Vec<Line<'static>> {
    model
        .activity
        .iter()
        .map(|entry| activity_line(model, entry, width))
        .collect()
}

fn activity_line(model: &Model, entry: &Entry, width: usize) -> Line<'static> {
    let time = entry
        .at
        .to_zoned(model.tz.clone())
        .strftime("%H:%M:%S")
        .to_string();
    let mark = match &entry.status {
        Status::Waiting => spinner(model),
        Status::Done(_) => Span::styled("✓", Style::new().fg(GOOD)),
        Status::Warn(_) => Span::styled("⚠", Style::new().fg(WARN)),
        Status::Failed(_) => Span::styled("✗", Style::new().fg(BAD)),
        Status::Info => Span::styled("·", DIM),
    };
    let tail = match &entry.status {
        Status::Done(Some(took)) => format!("  {}", seconds(took.as_secs_f64())),
        Status::Warn(why) | Status::Failed(why) => format!("  {why}"),
        _ => String::new(),
    };
    let tail_style = match &entry.status {
        Status::Failed(_) => Style::new().fg(BAD),
        Status::Warn(_) => Style::new().fg(WARN),
        _ => DIM,
    };
    let origin = match entry.origin {
        Origin::Telka => Span::styled("telka", DIM),
        Origin::Remote => Span::styled("remote", Style::new().fg(REMOTE)),
    };
    let fixed = 8 + 2 + 2 + WHAT + 1 + origin.width() + 2;
    let room = width.saturating_sub(fixed);
    let text = fit(&entry.text, room.min(entry.text.chars().count()));
    let tail = fit(
        &tail,
        room.saturating_sub(text.chars().count())
            .min(tail.chars().count()),
    );
    let line = spread(
        vec![
            Span::styled(time, DIM),
            Span::raw("  "),
            mark,
            Span::raw(" "),
            Span::styled(fit(entry.what, WHAT), BOLD),
            Span::raw(" "),
            Span::raw(text),
            Span::styled(tail, tail_style),
        ],
        vec![origin],
        width,
    );
    if model.flashing(entry) {
        line.style(Style::new().bg(SELECTED))
    } else {
        line
    }
}

fn seconds(seconds: f64) -> String {
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{seconds:.0} s")
    }
}

fn spinner(model: &Model) -> Span<'static> {
    let frame = usize::try_from(model.now.as_millisecond().rem_euclid(1000) / 100).unwrap_or(0);
    Span::styled(
        SPINNER[frame % SPINNER.len()].to_string(),
        Style::new().fg(WARN),
    )
}

/// `left`, then `right` flush with the end of `width` columns.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used: usize = left.iter().chain(&right).map(Span::width).sum();
    let mut spans = left;
    if !right.is_empty() {
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
        spans.extend(right);
    }
    Line::from(spans)
}

/// `text` shortened to `width` columns with an ellipsis, or padded to them.
fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count > width {
        let kept: String = text.chars().take(width.saturating_sub(1)).collect();
        format!("{kept}…")
    } else {
        format!("{text}{}", " ".repeat(width - count))
    }
}

fn centered(frame: &mut Frame, area: Rect, title: &str, text: Vec<Line<'static>>) {
    let width = text.iter().map(Line::width).max().unwrap_or(0) + 4;
    let width = u16::try_from(width)
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(2));
    let height = u16::try_from(text.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let popup = area.centered(Constraint::Length(width), Constraint::Length(height));
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(title.to_owned(), BOLD.fg(ACCENT)))
        .padding(Padding::horizontal(1));
    frame.render_widget(Paragraph::new(text).block(block), popup);
}

fn pairing(model: &Model, dialog: &Dialog, frame: &mut Frame, body: Rect) {
    let mut text = Vec::new();
    match dialog {
        Dialog::Searching { reason } => {
            if let Some(reason) = reason {
                text.push(Line::styled(reason.clone(), Style::new().fg(WARN)));
                text.push(Line::default());
            }
            text.push(Line::from(vec![
                spinner(model),
                Span::raw(" looking for the TV on the network"),
            ]));
        }
        Dialog::Prompt {
            address,
            signed,
            until,
        } => {
            text.push(Line::from(vec![
                Span::styled("✓", Style::new().fg(GOOD)),
                Span::raw(" found the TV "),
                Span::styled(format!("at {address}"), DIM),
            ]));
            let left = until.duration_since(model.now).as_secs().max(0);
            let asks = if *signed {
                " Accept telka on the TV: it asks for \"LG Remote App\""
            } else {
                " Accept the prompt on the TV with the remote"
            };
            text.push(Line::from(vec![
                spinner(model),
                Span::raw(asks),
                Span::styled(format!("   {left} s"), Style::new().fg(WARN)),
            ]));
        }
        Dialog::Failed(message) => {
            text.push(Line::styled(format!("✗ {message}"), Style::new().fg(BAD)));
        }
    }
    text.push(Line::default());
    text.push(Line::styled(
        "the key stays on this computer; `telka unpair` forgets it",
        DIM,
    ));
    centered(frame, body, " pair with the TV ", text);
}

fn statusline(model: &Model, frame: &mut Frame, area: Rect) {
    let (label, color) = match &model.phase {
        Phase::Pairing(_) => ("PAIRING", PRESS),
        Phase::Off { .. } => ("OFF", BAD),
        Phase::Waking { .. } => ("WAKE", WARN),
        Phase::Live => ("LIVE", GOOD),
        Phase::Connecting => ("CONNECTING", WARN),
    };
    let name = tv_title(model);
    // Each segment has the rank it is dropped in when the window is narrow: highest first.
    let mut segments: Vec<(u8, Vec<Span<'static>>)> = Vec::new();
    match (&model.phase, &model.tv) {
        (Phase::Off { retry_at, .. }, _) => {
            let left = retry_at.duration_since(model.now).as_secs().max(0);
            segments.push((
                0,
                vec![Span::styled(
                    format!("looking again in {left} s · p wakes it"),
                    Style::new().fg(BAD),
                )],
            ));
        }
        (_, Some(tv)) => {
            segments.push((1, vec![Span::raw(tv.address.to_string())]));
            if let Some(release) = &tv.release {
                segments.push((3, vec![Span::styled(format!("webOS {release}"), DIM)]));
            }
            let transport = if tv.secure { "wss" } else { "ws" };
            let manifest = if tv.signed { "signed" } else { "unsigned" };
            segments.push((2, vec![Span::raw(format!("{transport} · {manifest}"))]));
        }
        _ => {}
    }
    let line = |segments: &[(u8, Vec<Span<'static>>)]| {
        let mut spans = vec![
            Span::styled(format!(" {label} "), BOLD.fg(Color::Black).bg(color)),
            Span::styled(format!(" {name} "), BOLD.bg(MUTED)),
        ];
        for (index, (_, segment)) in segments.iter().enumerate() {
            spans.push(if index == 0 {
                " ".into()
            } else {
                Span::styled(" │ ", DIM)
            });
            spans.extend(segment.iter().cloned());
        }
        Line::from(spans)
    };
    let width = usize::from(area.width);
    while line(&segments).width() > width {
        let Some(least) = (0..segments.len()).max_by_key(|&i| segments[i].0) else {
            break;
        };
        segments.remove(least);
    }
    frame.render_widget(line(&segments), area);
}

fn hint_bar(model: &Model, frame: &mut Frame, area: Rect, shape: Shape) {
    // (rank, keys, label); a lower rank stays longer when the window is narrow.
    let hints: Vec<(u8, &str, &str)> = match (&model.overlay, &model.phase) {
        (Overlay::Help, _) => vec![(0, "Esc", "close"), (0, "q", "quit")],
        (Overlay::Picker(picker), _) => vec![
            (1, "type", "to filter"),
            (0, "↑↓", "select"),
            (
                0,
                "⏎",
                match picker.kind {
                    PickerKind::Apps => "open",
                    PickerKind::Inputs => "switch",
                    PickerKind::Buttons => "press",
                    PickerKind::Modes => "set",
                },
            ),
            (0, "Esc", "close"),
        ],
        (Overlay::Text(_), _) => vec![
            (0, "⏎", "Enter on the TV"),
            (0, "⌫", "delete"),
            (0, "Esc", "close"),
        ],
        (_, Phase::Pairing(Dialog::Failed(_))) => vec![(0, "⏎", "try again"), (0, "q", "quit")],
        (_, Phase::Pairing(_)) => vec![(0, "Esc", "cancel"), (0, "q", "quit")],
        (_, Phase::Off { .. }) => vec![
            (0, "p", "wake"),
            (1, "R", "look now"),
            (2, "?", "help"),
            (0, "q", "quit"),
        ],
        (_, Phase::Live) => BINDINGS
            .iter()
            .filter(|binding| match binding.hint {
                Hint::Always | Hint::Live => true,
                Hint::Tabs => shape == Shape::Tabs,
                Hint::Never => false,
            })
            .map(|binding| (binding.rank, binding.short, binding.label))
            .collect(),
        _ => vec![(0, "q", "quit")],
    };
    let right = match &model.notice {
        Some(notice) if notice.until > model.now => {
            Span::styled(format!("{} ", notice.text), Style::new().fg(WARN))
        }
        _ => Span::styled(concat!("telka ", env!("CARGO_PKG_VERSION"), " "), DIM),
    };
    let left = |hints: &[(u8, &str, &str)]| {
        let mut left = vec![Span::raw(" ")];
        for (index, (_, key, label)) in hints.iter().enumerate() {
            if index > 0 {
                left.push(Span::raw("  "));
            }
            left.push(Span::styled((*key).to_owned(), BOLD));
            left.push(Span::raw(format!(" {label}")));
        }
        left
    };
    let width = usize::from(area.width);
    // At least a few columns between the hints and what is on the right.
    let fits = |hints: &[(u8, &str, &str)]| {
        left(hints).iter().map(Span::width).sum::<usize>() + right.width() + 3 <= width
    };
    let mut hints = hints;
    while !fits(&hints) {
        let Some(least) = (0..hints.len()).max_by_key(|&i| hints[i].0) else {
            break;
        };
        hints.remove(least);
    }
    frame.render_widget(spread(left(&hints), vec![right], width), area);
}

/// Rows the picker shows at once.
const PICKER_ROWS: usize = 10;

fn picker_dialog(model: &Model, picker: &Picker, frame: &mut Frame, area: Rect) {
    let choices = model.choices();
    let total = match picker.kind {
        PickerKind::Apps => model.tv.as_ref().map_or(0, |tv| tv.apps.len()),
        PickerKind::Inputs => model.tv.as_ref().map_or(0, |tv| tv.inputs.len()),
        PickerKind::Buttons => webos::Button::all().count(),
        PickerKind::Modes => crate::control::PICTURE_MODES.len(),
    };
    let (title, noun) = match picker.kind {
        PickerKind::Apps => (" apps ", "apps"),
        PickerKind::Inputs => (" inputs ", "inputs"),
        PickerKind::Buttons => (" remote buttons ", "buttons"),
        PickerKind::Modes => (" picture mode ", "modes"),
    };
    let width = usize::from(area.width.saturating_sub(8)).min(64);
    let mut text = vec![
        Line::from(vec![
            Span::styled("filter  ", DIM),
            Span::raw(picker.filter.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]),
        Line::default(),
    ];
    let start = picker
        .selected
        .saturating_sub(PICKER_ROWS / 2)
        .min(choices.len().saturating_sub(PICKER_ROWS));
    let title_width = width.saturating_sub(4) * 3 / 5;
    for (index, choice) in choices.iter().enumerate().skip(start).take(PICKER_ROWS) {
        let selected = index == picker.selected;
        let mark = if selected {
            Span::styled("▸ ", Style::new().fg(ACCENT))
        } else {
            Span::raw("  ")
        };
        let dot = if choice.current {
            Span::styled("● ", Style::new().fg(GOOD))
        } else {
            Span::raw("  ")
        };
        let detail_width = width.saturating_sub(4 + title_width + 1);
        let mut line = Line::from(vec![
            mark,
            dot,
            Span::styled(
                fit(&choice.title, title_width),
                if selected { BOLD } else { Style::new() },
            ),
            Span::raw(" "),
            Span::styled(fit(&choice.detail, detail_width), DIM),
        ]);
        if selected {
            line = line.style(Style::new().bg(SELECTED));
        }
        text.push(line);
    }
    if choices.is_empty() {
        text.push(Line::styled("  nothing matches", DIM));
    }
    text.push(Line::default());
    text.push(Line::styled(
        format!("{} of {total} {noun}", choices.len()),
        DIM,
    ));
    centered(frame, area, title, text);
}

/// The line that types into the TV, at the bottom so the TV's state stays in view.
fn text_line(line: &TextLine, frame: &mut Frame, area: Rect) {
    let what = match line.field.as_deref() {
        Some("url") => "an address field on the TV has focus".to_owned(),
        Some(kind) if !kind.is_empty() => format!("a {kind} field on the TV has focus"),
        _ => "goes to the text field on the TV that has focus".to_owned(),
    };
    let shown = if line.hidden {
        "•".repeat(line.text.chars().count())
    } else {
        line.text.clone()
    };
    let width = usize::from(area.width.saturating_sub(8)).min(64);
    let text = vec![
        Line::styled(what, DIM),
        Line::default(),
        Line::from(vec![
            Span::styled("❯ ", Style::new().fg(ACCENT)),
            Span::raw(shown.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
            Span::raw(" ".repeat(width.saturating_sub(shown.chars().count() + 3))),
        ]),
    ];
    let height = u16::try_from(text.len() + 2).unwrap_or(u16::MAX);
    let popup_width = u16::try_from(width + 4).unwrap_or(u16::MAX).min(area.width);
    let x = area.x + area.width.saturating_sub(popup_width) / 2;
    // Above the status line and the hints.
    let y = area.bottom().saturating_sub(height + 3);
    let popup = Rect::new(x, y, popup_width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(" type on the TV ", BOLD.fg(ACCENT)))
        .padding(Padding::horizontal(1));
    frame.render_widget(Paragraph::new(text).block(block), popup);
}

fn help(frame: &mut Frame, area: Rect) {
    let mut text: Vec<Line<'static>> = BINDINGS
        .iter()
        .filter(|binding| !binding.keys.is_empty())
        .map(|binding| {
            Line::from(vec![
                Span::styled(fit(binding.keys, 10), BOLD),
                Span::raw(binding.help),
            ])
        })
        .collect();
    text.push(Line::default());
    text.push(Line::styled(
        "mouse: in the remote pane, a click on a button presses it; a drag",
        DIM,
    ));
    text.push(Line::styled(
        "moves the TV's pointer, a click clicks, the wheel scrolls; the",
        DIM,
    ));
    text.push(Line::styled("wheel over the volume turns it", DIM));
    centered(frame, area, " keys ", text);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use serde_json::json;
    use webos::{Report, Volume};

    use super::*;
    use crate::tui::model::tests::{key, later, live, snapshot, start};
    use crate::tui::model::{Change, Failure, Msg, PairingMsg};

    fn screen(model: &Model, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render(model, frame);
            })
            .unwrap();
        terminal.backend().to_string()
    }

    /// Live, with changes from telka and from elsewhere in the activity.
    fn lived_in() -> Model {
        let mut model = live();
        later(&mut model, 5_000);
        model.update(key(KeyCode::Up));
        model.update(key(KeyCode::Up));
        model.update(key(KeyCode::Enter));
        model.update(key(KeyCode::Char('+')));
        later(&mut model, 150);
        model.update(key(KeyCode::Char('+')));
        later(&mut model, 120);
        model.update(Msg::Changed(Change::Volume(
            Volume::read(
                &json!({"volumeStatus": {"volume": 14, "muteStatus": false, "maxVolume": 100}}),
            )
            .unwrap(),
        )));
        later(&mut model, 30_000);
        model.update(Msg::Changed(Change::App("netflix".into())));
        later(&mut model, 2_000);
        model
    }

    #[test]
    fn wide_layout() {
        insta::assert_snapshot!(screen(&lived_in(), 110, 26));
    }

    #[test]
    fn narrow_layout_with_tabs() {
        let mut model = lived_in();
        insta::assert_snapshot!("tab_tv", screen(&model, 70, 22));
        model.update(key(KeyCode::Tab));
        insta::assert_snapshot!("tab_activity", screen(&model, 70, 22));
    }

    #[test]
    fn too_small_a_window() {
        insta::assert_snapshot!(screen(&lived_in(), 40, 12));
    }

    #[test]
    fn an_app_picker_filters() {
        let mut model = lived_in();
        model.update(key(KeyCode::Char('a')));
        model.update(key(KeyCode::Char('o')));
        insta::assert_snapshot!(screen(&model, 110, 26));
    }

    #[test]
    fn help_overlay() {
        let mut model = lived_in();
        model.update(key(KeyCode::Char('?')));
        insta::assert_snapshot!(screen(&model, 110, 30));
    }

    #[test]
    fn the_tv_off_and_waking() {
        let mut model = lived_in();
        model.update(Msg::Lost);
        insta::assert_snapshot!("off", screen(&model, 110, 26));
        model.update(key(KeyCode::Char('p')));
        later(&mut model, 12_000);
        insta::assert_snapshot!("waking", screen(&model, 110, 26));
    }

    #[test]
    fn screen_off_and_a_turn_off_on_its_way() {
        let mut model = lived_in();
        model.update(Msg::Changed(Change::Power(
            webos::PowerState::read(&json!({"state": "Screen Off"})).unwrap(),
        )));
        model.update(key(KeyCode::Char('p')));
        insta::assert_snapshot!(screen(&model, 110, 26));
    }

    #[test]
    fn pairing_dialog() {
        let (mut model, _) = Model::start(start());
        model.update(Msg::Connected(Err(Failure::NotPaired)));
        insta::assert_snapshot!("pairing_searching", screen(&model, 110, 26));
        model.update(Msg::Pairing(PairingMsg::Prompt {
            address: "192.168.1.30".parse().unwrap(),
            signed: true,
        }));
        later(&mut model, 3_000);
        insta::assert_snapshot!("pairing_prompt", screen(&model, 110, 26));
        model.update(Msg::Pairing(PairingMsg::Failed(
            "pairing was declined on the TV".into(),
        )));
        insta::assert_snapshot!("pairing_failed", screen(&model, 110, 26));
    }

    #[test]
    fn the_line_that_types_into_the_tv() {
        let mut model = lived_in();
        model.update(Msg::Changed(Change::Keyboard(webos::Keyboard {
            focused: true,
            content_type: Some("url".into()),
            hidden: false,
        })));
        for c in "lg.com".chars() {
            model.update(key(KeyCode::Char(c)));
        }
        insta::assert_snapshot!(screen(&model, 110, 26));
    }

    #[test]
    fn the_button_picker_and_the_pointer() {
        let mut model = lived_in();
        model.update(Msg::Pointer(crate::tui::model::Pointer::Move {
            dx: 36,
            dy: -24,
        }));
        model.update(key(KeyCode::Char('k')));
        model.update(key(KeyCode::Char('r')));
        model.update(key(KeyCode::Char('e')));
        insta::assert_snapshot!(screen(&model, 110, 26));
    }

    #[test]
    fn clicks_land_on_the_buttons() {
        let model = lived_in();
        let mut terminal = Terminal::new(TestBackend::new(110, 26)).unwrap();
        let mut spots = Vec::new();
        terminal
            .draw(|frame| spots = render(&model, frame))
            .unwrap();
        let at = |x: u16, y: u16| {
            spots
                .iter()
                .rev()
                .find(|spot| spot.area.contains(ratatui::layout::Position::new(x, y)))
                .map(|spot| spot.target.clone())
        };
        // The remote pane starts after the 46 columns of the TV pane, its border and padding.
        let left = 46 + 2;
        assert_eq!(
            at(left + 11, 3),
            Some(Target::Act(Action::Press(Button::ENTER)))
        );
        assert_eq!(
            at(left + 28, 2),
            Some(Target::Act(Action::Press(Button::HOME)))
        );
        assert_eq!(at(left + 10, 6), Some(Target::Act(Action::PlayPause)));
        assert_eq!(at(5, 3), Some(Target::Volume));
        assert_eq!(at(left + 20, 2), Some(Target::Pad), "next to the buttons");
        let _ = snapshot();
    }
}
