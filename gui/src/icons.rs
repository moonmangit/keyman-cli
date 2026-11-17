//! Keyman's line icons: drawn on a 24-point grid with a 1.8-point round stroke,
//! tinted with `.color()`.

use rune::Icon;

macro_rules! icons {
    ($($name:ident = $body:literal;)*) => {$(
        pub static $name: Icon = Icon {
            name: stringify!($name),
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        };
    )*};
}

icons! {
    KEY = r#"<circle cx="7.5" cy="15.5" r="4.5"/><path d="M10.7 12.3 20 3M16.5 6.5l3 3M14 9l2 2"/>"#;
    SEARCH = r#"<circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/>"#;
    PLUS = r#"<path d="M12 5v14M5 12h14"/>"#;
    REFRESH = r#"<path d="M20 12a8 8 0 1 1-2.6-5.9"/><path d="M20 4v5h-5"/>"#;
    HELP = r#"<circle cx="12" cy="12" r="9"/><path d="M9.6 9.4a2.5 2.5 0 1 1 3.6 2.3c-.7.3-1.2 1-1.2 1.7v.4M12 17h.01"/>"#;
    GEAR = r#"<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="7"/><path d="M12 2.5V5M12 19v2.5M2.5 12H5M19 12h2.5M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8"/>"#;
    FOLDER = r#"<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2.5h8.5A1.5 1.5 0 0 1 21 9v9.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5z"/>"#;
    EYE = r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>"#;
    EYE_OFF = r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/><path d="M4 4l16 16"/>"#;
    COPY = r#"<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2"/><path d="M15.5 8.5V5.5A1.5 1.5 0 0 0 14 4H5.5A1.5 1.5 0 0 0 4 5.5V14a1.5 1.5 0 0 0 1.5 1.5h3"/>"#;
    PENCIL = r#"<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>"#;
    CHECK_CIRCLE = r#"<circle cx="12" cy="12" r="9"/><path d="m8 12.5 2.7 2.7L16 9.8"/>"#;
    ALERT = r#"<path d="M12 3.5 2.5 20h19z"/><path d="M12 10v4.5M12 17.2h.01"/>"#;
    CLOSE = r#"<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>"#;
    COMPARE = r#"<path d="M4 8h14l-3.5-3.5M20 16H6l3.5 3.5"/>"#;
    DOWNLOAD = r#"<path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 20h14"/>"#;
    UPLOAD = r#"<path d="M12 15V4M7.5 8.5 12 4l4.5 4.5M5 20h14"/>"#;
    TERMINAL = r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M12.5 15H17"/>"#;
    KEYBOARD = r#"<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6.5 10h.01M10 10h.01M14 10h.01M17.5 10h.01M7.5 14h9"/>"#;
    BOOK = r#"<path d="M4 5.5A1.5 1.5 0 0 1 5.5 4H12v16H5.5A1.5 1.5 0 0 1 4 18.5zM12 4h6.5A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5H12"/>"#;
    INFO = r#"<circle cx="12" cy="12" r="9"/><path d="M12 11v5.5M12 7.8h.01"/>"#;
    EXTERNAL = r#"<path d="M14 4h6v6M20 4l-8.5 8.5M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10"/>"#;
    LIST = r#"<path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01"/>"#;
    SLIDERS = r#"<path d="M4 7h9M19 7h1M4 17h3M13 17h7"/><circle cx="16" cy="7" r="2.5"/><circle cx="10" cy="17" r="2.5"/>"#;
    PALETTE = r#"<path d="M12 3a9 9 0 1 0 0 18c1 0 1.6-.7 1.6-1.5 0-1.2-1-1.4-1-2.5 0-1 .8-1.5 1.8-1.5H17a4 4 0 0 0 4-4C21 6.8 17 3 12 3z"/><path d="M7.5 11.5h.01M10 7.5h.01M14.5 7.5h.01"/>"#;
    SHIELD = r#"<path d="M12 3 19.5 6v5.5c0 4.5-3.2 8-7.5 9.5-4.3-1.5-7.5-5-7.5-9.5V6z"/>"#;
    FILE = r#"<path d="M6 3h8l5 5v12a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5"/>"#;
    EXPAND = r#"<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>"#;
    MERGE = r#"<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="12" r="2"/><path d="M6 7v10M6 9c0 3 3 3 6 3h4"/>"#;
    MORE = r#"<path d="M5.5 12h.01M12 12h.01M18.5 12h.01" stroke-width="3"/>"#;
    CHEVRON_RIGHT = r#"<path d="m9.5 6 6 6-6 6"/>"#;
}
