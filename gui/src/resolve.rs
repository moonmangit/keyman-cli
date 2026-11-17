//! The conflict resolver: when a tied folder's .env disagrees with the master
//! file, pick the value each variable keeps (master, .env, or typed), then
//! write it to both so the folder and the project are in sync.

use keyman_cli::ui::display_path;
use rune::{Color, El, ScrollSpec, col, el, icon, row, text, text_input};

use crate::i18n::{count, t};
use crate::icons as ic;
use crate::widgets::*;
use crate::{App, Pick};

const MASK: &str = "••••••••";

/// One of the three choices for a variable.
fn chip(
    p: Paint,
    label: &str,
    value: Option<&str>,
    reveal: bool,
    current: Pick,
    i: usize,
    pick: Pick,
) -> El {
    let selected = current == pick;
    let key = format!("pick-{label}-{i}");
    let shown = match value {
        None if pick == Pick::Edit => t("Type a new value", "พิมพ์ค่าใหม่").to_owned(),
        None => t("Not set · removes it", "ไม่มีค่า · จะถูกลบ").to_owned(),
        Some(_) if !reveal => MASK.to_owned(),
        Some("") => t("(empty)", "(ว่าง)").to_owned(),
        Some(v) => v.to_owned(),
    };
    let ink = if value.is_some() && pick != Pick::Edit {
        p.text
    } else {
        p.muted
    };
    let mut c = col()
        .key(key)
        .role("radio")
        .label(label)
        .checked(selected)
        .focusable(true)
        .grow(1.0)
        .min_w(0.0)
        .px(10.0)
        .py(8.0)
        .gap(3.0)
        .radius(8.0)
        .border(
            if selected { 1.5 } else { 1.0 },
            if selected { p.accent } else { p.border },
        )
        .on_press(move |s: &mut App| {
            if let Some(r) = &mut s.resolver
                && let Some(item) = r.items.get_mut(i)
            {
                item.pick = pick;
            }
        })
        .child(
            row()
                .gap(6.0)
                .center_y()
                .child(
                    el().size(12.0, 12.0)
                        .radius(6.0)
                        .border(
                            if selected { 4.0 } else { 1.5 },
                            if selected { p.accent } else { p.faint },
                        )
                        .a11y_hidden(),
                )
                .child(text(label.to_owned()).font(CAPS).color(if selected {
                    p.accent
                } else {
                    p.faint
                })),
        )
        .child(text(shown).font(MONO_SMALL).color(ink).truncate());
    c = if selected {
        c.fill(p.accent.alpha(if p.dark { 0.12 } else { 0.07 }))
    } else {
        c.hover(p.hover)
    };
    c
}

pub fn resolve_dialog(s: &App, p: Paint) -> El {
    let Some(r) = &s.resolver else { return el() };
    let project = s.selected.clone().unwrap_or_default();
    let folder_name = r
        .folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n_items = r.items.len();

    let mut list = col().key("resolve-list").w_full().gap(10.0);
    for (i, item) in r.items.iter().enumerate() {
        let c = &item.conflict;
        let (kind, tint) = match (&c.master, &c.env) {
            (Some(_), Some(_)) => (t("Changed", "เปลี่ยนค่า"), p.yellow),
            (None, Some(_)) => (t("Only in .env", "มีเฉพาะใน .env"), p.green),
            (Some(_), None) => (t("Only in master", "มีเฉพาะในไฟล์หลัก"), p.red),
            (None, None) => ("", p.faint),
        };
        let mut entry = col()
            .key(format!("conflict-{}", c.name))
            .w_full()
            .padding(12.0)
            .gap(10.0)
            .radius(10.0)
            .fill(p.card)
            .border(1.0, p.border)
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(
                        text(c.name.clone())
                            .font(MONO)
                            .color(p.text)
                            .truncate()
                            .min_w(0.0),
                    )
                    .child(badge(p, kind, tint)),
            )
            .child(
                row()
                    .key("choices")
                    .w_full()
                    .gap(8.0)
                    .child(chip(
                        p,
                        t("Master", "ไฟล์หลัก"),
                        c.master.as_deref(),
                        r.reveal,
                        item.pick,
                        i,
                        Pick::Master,
                    ))
                    .child(chip(
                        p,
                        ".env",
                        c.env.as_deref(),
                        r.reveal,
                        item.pick,
                        i,
                        Pick::Env,
                    ))
                    .child(chip(
                        p,
                        t("Edit", "แก้ไข"),
                        None,
                        r.reveal,
                        item.pick,
                        i,
                        Pick::Edit,
                    )),
            );
        if item.pick == Pick::Edit {
            let mut field = text_input(&item.edit, MONO)
                .key(format!("edit-{i}"))
                .label(tf!("New value for {}", "ค่าใหม่ของ {}", c.name))
                .single_line()
                .placeholder(t("Value", "ค่า"), p.faint)
                .color(p.text)
                .on_edit(move |s: &mut App| s.resolver_edit(i))
                .grow(1.0)
                .min_w(0.0);
            if !r.reveal {
                field = field.secure();
            }
            entry = entry.child(
                row()
                    .key("edit-row")
                    .w_full()
                    .h(34.0)
                    .px(10.0)
                    .center_y()
                    .radius(8.0)
                    .fill(if p.dark {
                        Color::BLACK.alpha(0.2)
                    } else {
                        Color::WHITE
                    })
                    .border(1.0, p.accent.alpha(0.6))
                    .child(field),
            );
        }
        list = list.child(entry);
    }

    let keeps_env = r.items.iter().filter(|it| it.pick != Pick::Master).count();
    let outcome = if keeps_env == 0 {
        tf!(
            "{}/.env will be rewritten to match {}.",
            "{}/.env จะถูกเขียนใหม่ให้ตรงกับ {}",
            folder_name,
            project
        )
    } else {
        tf!(
            "{} will change in the master file; {}/.env will match it.",
            "{} จะเปลี่ยนในไฟล์หลัก และ {}/.env จะตรงกัน",
            count(keeps_env, "variable", "variables", "ตัวแปร"),
            folder_name
        )
    };

    col()
        .key("resolve")
        .role("dialog")
        .label(t("Resolve differences", "แก้ไขความต่าง"))
        .w(720.0)
        .max_h(620.0)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.border)
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
        .clip_rounded(14.0)
        .child(
            row()
                .key("resolve-head")
                .w_full()
                .padding(20.0)
                .gap(14.0)
                .border_sides([0.0, 0.0, 1.0, 0.0], p.separator)
                .child(icon_tile(&ic::MERGE, 40.0, p.yellow, p.dark))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(4.0)
                        .child(text(tf!("Sync {} with {}", "ซิงก์ {} กับ {}", display_path(&r.folder), project)).font(H2).color(p.text).truncate())
                        .child(
                            text(tf!(
                                "{} between the master file and {}. Pick the value each one keeps; both files are updated to match.",
                                "มี {} ระหว่างไฟล์หลักและ {} เลือกค่าที่จะเก็บของแต่ละตัว แล้วทั้งสองไฟล์จะถูกอัปเดตให้ตรงกัน",
                                count(n_items, "difference", "differences", "จุดที่ต่าง"),
                                display_path(&r.folder.join(".env"))
                            ))
                            .font(SMALL)
                            .color(p.muted)
                            .wrap(),
                        ),
                )
                .child(icon_button(
                    p,
                    "resolve-reveal",
                    if r.reveal { &ic::EYE_OFF } else { &ic::EYE },
                    if r.reveal { t("Hide values", "ซ่อนค่า") } else { t("Show values", "แสดงค่า") },
                    |s, _| {
                        if let Some(r) = &mut s.resolver {
                            r.reveal = !r.reveal;
                        }
                    },
                )),
        )
        .child(
            row()
                .key("resolve-bulk")
                .w_full()
                .px(20.0)
                .pt(12.0)
                .gap(8.0)
                .center_y()
                .child(text(t("Use for all:", "ใช้กับทั้งหมด:")).font(SMALL).color(p.muted))
                .child(button(p, "all-master", t("Master", "ไฟล์หลัก"), Some(&ic::FILE), Kind::Secondary, true, |s, _| s.pick_all(Pick::Master)))
                .child(button(p, "all-env", ".env", Some(&ic::FOLDER), Kind::Secondary, true, |s, _| s.pick_all(Pick::Env))),
        )
        .child(
            col()
                .key("resolve-scroll")
                .w_full()
                .px(20.0)
                .py(12.0)
                .child(list)
                .scroll(ScrollSpec { stick_bottom: false })
                .shrink()
                .min_h(0.0),
        )
        .child(
            row()
                .key("resolve-foot")
                .w_full()
                .px(20.0)
                .py(14.0)
                .gap(10.0)
                .center_y()
                .border_sides([1.0, 0.0, 0.0, 0.0], p.separator)
                .child(icon(&ic::INFO, 14.0).color(p.faint))
                .child(text(outcome).font(SMALL).color(p.muted).grow(1.0).min_w(0.0).truncate())
                .child(button(p, "resolve-cancel", t("Later", "ไว้ทีหลัง"), None, Kind::Ghost, true, |s, _| {
                    s.dialog = None;
                    s.resolver = None;
                }))
                .child(button(
                    p,
                    "resolve-apply",
                    &tf!("Sync {}", "ซิงก์ {}", count(n_items, "variable", "variables", "ตัวแปร")),
                    Some(&ic::MERGE),
                    Kind::Primary,
                    true,
                    |s, _| s.apply_resolution(),
                )),
        )
}
