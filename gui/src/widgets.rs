//! Keyman's look: one palette for light and dark, and the controls built on it.

use rune::tok;
use rune::{Color, Cursor, Drag, El, Icon, Phase, TextStyle, Tx, col, el, icon, oklch, row, text};

use crate::App;

pub const TITLE_H: f32 = 44.0;
pub const H1: TextStyle = TextStyle::new(22.0, 650.0);
pub const H2: TextStyle = TextStyle::new(15.0, 600.0);
pub const BODY: TextStyle = TextStyle::new(13.0, 400.0);
pub const BODY_STRONG: TextStyle = TextStyle::new(13.0, 550.0);
pub const SMALL: TextStyle = TextStyle::new(12.0, 400.0);
pub const CAPS: TextStyle = TextStyle::new(11.0, 600.0).tracking(0.4);
pub const MONO: TextStyle = TextStyle::new(12.5, 400.0).mono();
pub const MONO_SMALL: TextStyle = TextStyle::new(11.5, 400.0).mono();

#[derive(Clone, Copy)]
pub struct Paint {
    pub dark: bool,
    pub bg: Color,
    pub panel: Color,
    pub card: Color,
    pub raised: Color,
    pub border: Color,
    pub separator: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub hover: Color,
    pub press: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub shadow: Color,
}

impl Paint {
    pub fn new(dark: bool, accent: usize) -> Paint {
        let tint = 265.0;
        let accent = tok::accent(accent, dark);
        if dark {
            Paint {
                dark,
                bg: oklch(0.175, 0.006, tint),
                panel: oklch(0.2, 0.007, tint),
                card: oklch(0.215, 0.007, tint),
                raised: oklch(0.25, 0.008, tint),
                border: Color::WHITE.alpha(0.085),
                separator: Color::WHITE.alpha(0.06),
                text: oklch(0.95, 0.0, 0.0),
                muted: oklch(0.72, 0.01, tint),
                faint: oklch(0.55, 0.01, tint),
                hover: Color::WHITE.alpha(0.06),
                press: Color::WHITE.alpha(0.1),
                accent,
                on_accent: Color::WHITE,
                red: oklch(0.7, 0.17, 25.0),
                green: oklch(0.74, 0.15, 155.0),
                yellow: oklch(0.82, 0.13, 85.0),
                shadow: Color::BLACK.alpha(0.35),
            }
        } else {
            Paint {
                dark,
                bg: oklch(0.985, 0.002, tint),
                panel: oklch(0.96, 0.004, tint),
                card: Color::WHITE,
                raised: Color::WHITE,
                border: Color::BLACK.alpha(0.09),
                separator: Color::BLACK.alpha(0.06),
                text: oklch(0.22, 0.01, tint),
                muted: oklch(0.47, 0.012, tint),
                faint: oklch(0.62, 0.01, tint),
                hover: Color::BLACK.alpha(0.045),
                press: Color::BLACK.alpha(0.08),
                accent,
                on_accent: Color::WHITE,
                red: oklch(0.56, 0.19, 25.0),
                green: oklch(0.56, 0.14, 155.0),
                yellow: oklch(0.62, 0.13, 70.0),
                shadow: Color::BLACK.alpha(0.12),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// A labelled button, with an optional leading icon.
/// A page-header button: its label and icon, or only its icon (the label as
/// a tooltip) when the page is narrow.
pub fn action_button(
    p: Paint,
    key: &str,
    label: &str,
    glyph: &'static Icon,
    enabled: bool,
    f: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    if !crate::views::narrow() {
        return button(p, key, label, Some(glyph), Kind::Secondary, enabled, f);
    }
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .help(label)
        .size(30.0, 30.0)
        .center()
        .radius(7.0)
        .fill(p.raised)
        .border(1.0, p.border)
        .shadow(1.0, 1.5, p.shadow.alpha(0.25))
        .disabled(!enabled)
        .child(icon(glyph, 15.0).color(p.text).a11y_hidden());
    if enabled {
        b.focusable(true)
            .hover(p.hover)
            .press(p.press)
            .on_press_tx(f)
    } else {
        b.opacity(0.4)
    }
}

pub fn button(
    p: Paint,
    key: &str,
    label: &str,
    glyph: Option<&'static Icon>,
    kind: Kind,
    enabled: bool,
    f: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    let (fill, ink, border) = match kind {
        Kind::Primary => (p.accent, p.on_accent, Color::TRANSPARENT),
        Kind::Danger => (p.red, Color::WHITE, Color::TRANSPARENT),
        Kind::Secondary => (p.raised, p.text, p.border),
        Kind::Ghost => (Color::TRANSPARENT, p.text, Color::TRANSPARENT),
    };
    let hover = if matches!(kind, Kind::Primary | Kind::Danger) {
        Color::WHITE.alpha(0.12)
    } else {
        p.hover
    };
    let mut b = row()
        .key(key)
        .role("button")
        .label(label)
        .h(30.0)
        .px(12.0)
        .gap(6.0)
        .center()
        .radius(7.0)
        .fill(fill)
        .border(1.0, border)
        .disabled(!enabled);
    if let Some(glyph) = glyph {
        b = b.child(icon(glyph, 15.0).color(ink).a11y_hidden());
    }
    b = b.child(
        text(label.to_owned())
            .font(BODY_STRONG)
            .color(ink)
            .a11y_hidden(),
    );
    if kind != Kind::Ghost {
        b = b.shadow(1.0, 1.5, p.shadow.alpha(0.25));
    }
    if enabled {
        b.focusable(true).hover(hover).press(p.press).on_press_tx(f)
    } else {
        b.opacity(0.45)
    }
}

/// A square icon button with a tooltip.
pub fn icon_button(
    p: Paint,
    key: &str,
    glyph: &'static Icon,
    tip: &str,
    f: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(tip)
        .help(tip)
        .size(30.0, 30.0)
        .center()
        .radius(7.0)
        .focusable(true)
        .hover(p.hover)
        .press(p.press)
        .on_press_tx(f)
        .child(icon(glyph, 17.0).color(p.muted).a11y_hidden())
}

/// A small coloured label.
pub fn badge(p: Paint, label: &str, tint: Color) -> El {
    row()
        .h(20.0)
        .px(8.0)
        .center()
        .radius(10.0)
        .fill(tint.alpha(if p.dark { 0.16 } else { 0.12 }))
        .child(
            text(label.to_owned())
                .font(TextStyle::new(11.0, 600.0))
                .color(tint),
        )
}

/// A keyboard shortcut key cap.
pub fn kbd(p: Paint, label: &str) -> El {
    row()
        .h(22.0)
        .min_w(22.0)
        .px(6.0)
        .center()
        .radius(5.0)
        .fill(p.raised)
        .border(1.0, p.border)
        .shadow(1.0, 0.0, p.border)
        .child(
            text(label.to_owned())
                .font(TextStyle::new(11.5, 500.0))
                .color(p.muted),
        )
}

pub fn card(p: Paint, key: &str) -> El {
    col()
        .key(key)
        .w_full()
        .radius(12.0)
        .fill(p.card)
        .border(1.0, p.border)
        .shadow(1.0, 2.0, p.shadow.alpha(0.4))
}

pub fn hairline(p: Paint, key: impl Into<String>) -> El {
    el().key(key)
        .h(1.0)
        .w_full()
        .fill(p.separator)
        .a11y_hidden()
}

/// A row of mutually exclusive choices.
pub fn segmented<T: Copy + PartialEq + 'static>(
    p: Paint,
    key: &str,
    options: &[(T, &'static str, &'static str)],
    current: T,
    set: fn(&mut App, T),
) -> El {
    let mut r = row()
        .key(key)
        .role("radiogroup")
        .h(30.0)
        .padding(2.0)
        .gap(2.0)
        .radius(8.0)
        .fill(if p.dark {
            Color::BLACK.alpha(0.25)
        } else {
            Color::BLACK.alpha(0.05)
        });
    for &(value, en, th) in options {
        let name = crate::i18n::t(en, th);
        let selected = value == current;
        let mut seg = row()
            .key(format!("{key}-{en}"))
            .role("radio")
            .label(name)
            .checked(selected)
            .focusable(true)
            .h_full()
            .px(12.0)
            .center()
            .radius(6.0)
            .on_press(move |s: &mut App| set(s, value))
            .child(
                text(name.to_owned())
                    .font(if selected { BODY_STRONG } else { BODY })
                    .color(if selected { p.text } else { p.muted })
                    .a11y_hidden(),
            );
        seg = if selected {
            seg.fill(p.raised).shadow(1.0, 2.0, p.shadow.alpha(0.5))
        } else {
            seg.hover(p.hover)
        };
        r = r.child(seg);
    }
    r
}

/// An on/off switch.
pub fn switch(p: Paint, key: &str, label: &str, on: bool, flip: fn(&mut App)) -> El {
    let (w, h, k) = (36.0, 20.0, 16.0);
    let inset = (h - k) / 2.0;
    el().key(key)
        .role("switch")
        .label(label)
        .checked(on)
        .focusable(true)
        .size(w, h)
        .radius(h / 2.0)
        .fill(if on {
            p.accent
        } else if p.dark {
            Color::WHITE.alpha(0.16)
        } else {
            Color::BLACK.alpha(0.14)
        })
        .motion(tok::motion::SNAPPY)
        .on_press(flip)
        .child(
            el().key("knob")
                .size(k, k)
                .absolute(if on { w - k - inset } else { inset }, inset)
                .radius(k / 2.0)
                .fill(Color::WHITE)
                .shadow(1.0, 2.0, Color::BLACK.alpha(0.25))
                .a11y_hidden(),
        )
}

/// The six accent colours, as round swatches.
pub fn swatches(p: Paint, current: usize, set: fn(&mut App, usize)) -> El {
    let mut r = row()
        .key("swatches")
        .role("radiogroup")
        .label(crate::i18n::t("Accent colour", "สีเน้น"))
        .gap(10.0);
    for (i, name) in tok::ACCENT_NAMES.iter().enumerate() {
        let selected = i == current;
        r = r.child(
            el().key(format!("swatch-{name}"))
                .role("radio")
                .label(*name)
                .help(*name)
                .checked(selected)
                .focusable(true)
                .size(24.0, 24.0)
                .center()
                .radius(12.0)
                .border(
                    2.0,
                    if selected {
                        tok::accent(i, p.dark)
                    } else {
                        Color::TRANSPARENT
                    },
                )
                .on_press(move |s: &mut App| set(s, i))
                .child(
                    el().size(16.0, 16.0)
                        .radius(8.0)
                        .fill(tok::accent(i, p.dark))
                        .a11y_hidden(),
                ),
        );
    }
    r
}

/// One setting: a title and description on the left, its control on the right.
pub fn setting_row(p: Paint, key: &str, title: &str, description: &str, control: El) -> El {
    row()
        .key(key)
        .w_full()
        .py(12.0)
        .gap(16.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(text(title.to_owned()).font(BODY_STRONG).color(p.text))
                .children((!description.is_empty()).then(|| {
                    text(description.to_owned())
                        .font(SMALL)
                        .color(p.muted)
                        .wrap()
                })),
        )
        .child(control)
}

/// A titled group of setting rows separated by hairlines.
pub fn settings_group(p: Paint, key: &str, title: &str, rows: Vec<El>) -> El {
    let mut body = col()
        .key(format!("{key}-rows"))
        .w_full()
        .px(16.0)
        .radius(10.0)
        .fill(p.card)
        .border(1.0, p.border);
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        body = body.child(r);
        if i + 1 < n {
            body = body.child(hairline(p, format!("{key}-sep-{i}")));
        }
    }
    col()
        .key(key)
        .w_full()
        .gap(8.0)
        .child(text(title.to_owned()).font(CAPS).color(p.faint))
        .child(body)
}

/// A square tile holding an icon, for headers and list leaders.
pub fn icon_tile(glyph: &'static Icon, size: f32, tint: Color, dark: bool) -> El {
    el().size(size, size)
        .center()
        .radius(size * 0.28)
        .fill(tint.alpha(if dark { 0.18 } else { 0.12 }))
        .child(icon(glyph, size * 0.55).color(tint))
}

/// The status mark used beside projects and in banners.
pub fn dot(color: Color) -> El {
    el().size(8.0, 8.0).radius(4.0).fill(color).a11y_hidden()
}

/// A section whose width a divider changes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pane {
    Sidebar,
    Stages,
    /// Variables against Folders; the value is the width the two columns share.
    Split(f32),
}

/// A vertical divider that resizes `pane`: drag it, or double-click it to reset
/// the width. `line` draws a hairline in it (the borders between the side
/// columns); without it only the hover bar shows (the gap between the cards).
pub fn resizer(p: Paint, key: &str, pane: Pane, label: &str, line: bool) -> El {
    let mut handle = col()
        .key(key)
        .role("separator")
        .label(label)
        .help(crate::i18n::t(
            "Drag to resize · double-click to reset",
            "ลากเพื่อปรับขนาด · ดับเบิลคลิกเพื่อคืนค่าเดิม",
        ))
        .w(RESIZER_W)
        .h_full()
        .center()
        .cursor(Cursor::ResizeColumn)
        .hover(p.accent.alpha(0.45))
        .press(p.accent.alpha(0.7))
        .on_drag(
            move |s: &mut App, d: &mut Drag, _tx: &mut Tx| match d.phase {
                Phase::Down => d.data = s.pane_width(pane),
                Phase::Move => s.set_pane_width(pane, d.data + d.dx / s.settings.zoom),
                Phase::Up if d.cancelled() => s.set_pane_width(pane, d.data),
                Phase::Up => s.store_settings(),
            },
        )
        .on_double_press(move |s: &mut App| {
            s.reset_pane_width(pane);
            s.store_settings();
        });
    if line {
        handle = handle.child(el().w(1.0).h_full().fill(p.border));
    }
    handle
}

/// How wide a divider's grab area is.
pub const RESIZER_W: f32 = 5.0;
