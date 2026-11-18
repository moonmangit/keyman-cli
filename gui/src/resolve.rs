//! The conflict resolver: when a tied folder's .env disagrees with the master
//! file, pick the value each variable keeps (master, .env, or typed), then
//! write it to both so the folder and the project are in sync.
//!
//! The same dialog answers a cloud conflict during **Sync** (cloud mode): the sides are
//! "This computer" and "Cloud", and the answer goes to the waiting cloud task.

use keyman_cli::ui::display_path;
use rune::{Color, El, ScrollSpec, col, el, icon, row, text, text_input};

use crate::cloud::Ask;
use crate::i18n::{count, t};
use crate::icons as ic;
use crate::widgets::*;
use crate::{App, Pick, ResolveItem};

const MASK: &str = "••••••••";

/// Which resolver a dialog shows: a tied folder (`App::resolver`) or a cloud conflict
/// (`App::cloud_picks`).
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Folder,
    Cloud,
}

impl Mode {
    /// Keys of the cloud dialog are apart from the folder dialog's: both can show at once.
    fn prefix(self) -> &'static str {
        match self {
            Mode::Folder => "",
            Mode::Cloud => "cloud-",
        }
    }
}

fn items_mut(s: &mut App, mode: Mode) -> Option<&mut Vec<ResolveItem>> {
    match mode {
        Mode::Folder => s.resolver.as_mut().map(|r| &mut r.items),
        Mode::Cloud => s.cloud_picks.as_mut().map(|r| &mut r.items),
    }
}

fn toggle_reveal(s: &mut App, mode: Mode) {
    match mode {
        Mode::Folder => {
            if let Some(r) = &mut s.resolver {
                r.reveal = !r.reveal;
            }
        }
        Mode::Cloud => {
            if let Some(r) = &mut s.cloud_picks {
                r.reveal = !r.reveal;
            }
        }
    }
}

/// One of the three choices for a variable.
#[allow(clippy::too_many_arguments)]
fn chip(
    p: Paint,
    mode: Mode,
    label: &str,
    edit_hint: &str,
    value: Option<&str>,
    reveal: bool,
    current: Pick,
    i: usize,
    pick: Pick,
) -> El {
    let selected = current == pick;
    let key = format!("{}pick-{label}-{i}", mode.prefix());
    let shown = match value {
        None if pick == Pick::Edit => edit_hint.to_owned(),
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
            if let Some(items) = items_mut(s, mode)
                && let Some(item) = items.get_mut(i)
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

/// The cards of the conflicting variables, each with its three choices.
fn conflict_list(p: Paint, mode: Mode, items: &[ResolveItem], reveal: bool) -> El {
    let prefix = mode.prefix();
    let (first, second, third, edit_hint) = match mode {
        Mode::Folder => (
            t("Master", "ไฟล์หลัก"),
            ".env",
            t("Edit", "แก้ไข"),
            t("Type a new value", "พิมพ์ค่าใหม่"),
        ),
        Mode::Cloud => (
            t("This computer", "เครื่องนี้"),
            t("Cloud", "คลาวด์"),
            t("Type a new value", "พิมพ์ค่าใหม่"),
            t("Your own value", "ค่าของคุณเอง"),
        ),
    };
    let mut list = col()
        .key(format!("{prefix}resolve-list"))
        .w_full()
        .gap(10.0);
    for (i, item) in items.iter().enumerate() {
        let c = &item.conflict;
        let (kind, tint) = match (mode, &c.master, &c.env) {
            (Mode::Folder, Some(_), Some(_)) => (t("Changed", "เปลี่ยนค่า"), p.yellow),
            (Mode::Folder, None, Some(_)) => (t("Only in .env", "มีเฉพาะใน .env"), p.green),
            (Mode::Folder, Some(_), None) => (t("Only in master", "มีเฉพาะในไฟล์หลัก"), p.red),
            (Mode::Cloud, Some(_), Some(_)) => {
                (t("Changed on both sides", "เปลี่ยนทั้งสองฝั่ง"), p.yellow)
            }
            (Mode::Cloud, None, Some(_)) => (t("Removed on this computer", "ถูกลบในเครื่องนี้"), p.red),
            (Mode::Cloud, Some(_), None) => (t("Removed in the cloud", "ถูกลบในคลาวด์"), p.red),
            (_, None, None) => ("", p.faint),
        };
        let mut entry = col()
            .key(format!("{prefix}conflict-{}", c.name))
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
                        mode,
                        first,
                        edit_hint,
                        c.master.as_deref(),
                        reveal,
                        item.pick,
                        i,
                        Pick::Master,
                    ))
                    .child(chip(
                        p,
                        mode,
                        second,
                        edit_hint,
                        c.env.as_deref(),
                        reveal,
                        item.pick,
                        i,
                        Pick::Env,
                    ))
                    .child(chip(
                        p,
                        mode,
                        third,
                        edit_hint,
                        None,
                        reveal,
                        item.pick,
                        i,
                        Pick::Edit,
                    )),
            );
        if item.pick == Pick::Edit {
            let mut field = text_input(&item.edit, MONO)
                .key(format!("{prefix}edit-{i}"))
                .label(tf!("New value for {}", "ค่าใหม่ของ {}", c.name))
                .single_line()
                .placeholder(t("Value", "ค่า"), p.faint);
            field = match mode {
                Mode::Folder => field
                    .color(p.text)
                    .on_edit(move |s: &mut App| s.resolver_edit(i)),
                Mode::Cloud => field
                    .color(p.text)
                    .on_edit(move |s: &mut App| s.cloud_pick_edit(i)),
            };
            field = field.grow(1.0).min_w(0.0);
            field = match mode {
                Mode::Folder if !reveal => field.secure(),
                Mode::Folder => field,
                // A cloud value is always a secure field; reveal only shows its characters.
                Mode::Cloud => field.secure().revealed(reveal),
            };
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
    list
}

/// The parts of a resolver dialog that differ between the modes.
struct Sheet {
    mode: Mode,
    title: String,
    subtitle: String,
    reveal: bool,
    list: El,
    bulk: [El; 2],
    outcome: String,
    buttons: [El; 2],
}

fn sheet(p: Paint, sheet: Sheet) -> El {
    let Sheet {
        mode,
        title,
        subtitle,
        reveal,
        list,
        bulk: [bulk_first, bulk_second],
        outcome,
        buttons: [cancel, apply],
    } = sheet;
    let key = match mode {
        Mode::Folder => "resolve",
        Mode::Cloud => "cloud-resolve",
    };
    col()
        .key(key)
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
                .key(format!("{key}-head"))
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
                        .child(text(title).font(H2).color(p.text).truncate())
                        .child(text(subtitle).font(SMALL).color(p.muted).wrap()),
                )
                .child(icon_button(
                    p,
                    &format!("{key}-reveal"),
                    if reveal { &ic::EYE_OFF } else { &ic::EYE },
                    if reveal {
                        t("Hide values", "ซ่อนค่า")
                    } else {
                        t("Show values", "แสดงค่า")
                    },
                    move |s, _| toggle_reveal(s, mode),
                )),
        )
        .child(
            row()
                .key(format!("{key}-bulk"))
                .w_full()
                .px(20.0)
                .pt(12.0)
                .gap(8.0)
                .center_y()
                .child(
                    text(t("Use for all:", "ใช้กับทั้งหมด:"))
                        .font(SMALL)
                        .color(p.muted),
                )
                .child(bulk_first)
                .child(bulk_second),
        )
        .child(
            col()
                .key(format!("{key}-scroll"))
                .w_full()
                .px(20.0)
                .py(12.0)
                .child(list)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .shrink()
                .min_h(0.0),
        )
        .child(
            row()
                .key(format!("{key}-foot"))
                .w_full()
                .px(20.0)
                .py(14.0)
                .gap(10.0)
                .center_y()
                .border_sides([1.0, 0.0, 0.0, 0.0], p.separator)
                .child(icon(&ic::INFO, 14.0).color(p.faint))
                .child(
                    text(outcome)
                        .font(SMALL)
                        .color(p.muted)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(cancel)
                .child(apply),
        )
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

    // A cloud task may change the master file while this dialog is open: Apply waits for it.
    let idle = crate::cloud::can_start(&s.busy);
    let outcome = if idle {
        outcome
    } else {
        crate::cloud::folder_waits_text().to_owned()
    };

    sheet(
        p,
        Sheet {
            mode: Mode::Folder,
            title: tf!(
                "Sync {} with {}",
                "ซิงก์ {} กับ {}",
                display_path(&r.folder),
                project
            ),
            subtitle: tf!(
                "{} between the master file and {}. Pick the value each one keeps; both files are updated to match.",
                "มี {} ระหว่างไฟล์หลักและ {} เลือกค่าที่จะเก็บของแต่ละตัว แล้วทั้งสองไฟล์จะถูกอัปเดตให้ตรงกัน",
                count(n_items, "difference", "differences", "จุดที่ต่าง"),
                display_path(&r.folder.join(".env"))
            ),
            reveal: r.reveal,
            list: conflict_list(p, Mode::Folder, &r.items, r.reveal),
            bulk: [
                button(
                    p,
                    "all-master",
                    t("Master", "ไฟล์หลัก"),
                    Some(&ic::FILE),
                    Kind::Secondary,
                    true,
                    |s, _| s.pick_all(Pick::Master),
                ),
                button(
                    p,
                    "all-env",
                    ".env",
                    Some(&ic::FOLDER),
                    Kind::Secondary,
                    true,
                    |s, _| s.pick_all(Pick::Env),
                ),
            ],
            outcome,
            buttons: [
                button(
                    p,
                    "resolve-cancel",
                    t("Later", "ไว้ทีหลัง"),
                    None,
                    Kind::Ghost,
                    true,
                    |s, _| {
                        s.dialog = None;
                        s.resolver = None;
                    },
                ),
                button(
                    p,
                    "resolve-apply",
                    &tf!(
                        "Sync {}",
                        "ซิงก์ {}",
                        count(n_items, "variable", "variables", "ตัวแปร")
                    ),
                    Some(&ic::MERGE),
                    Kind::Primary,
                    idle,
                    |s, _| s.apply_resolution(),
                ),
            ],
        },
    )
}

/// Every cloud conflict keeps the same side.
fn pick_all_cloud(s: &mut App, pick: Pick) {
    if let Some(items) = items_mut(s, Mode::Cloud) {
        for item in items {
            item.pick = pick;
        }
    }
}

/// The cloud mode: a conflict between this computer and the cloud during **Sync** or
/// **Merge**. `None` when no cloud conflict waits.
pub fn cloud_resolve_dialog(s: &App, p: Paint) -> Option<El> {
    let Some(Ask::Resolve { project, .. }) = s.question.as_ref().map(|q| &q.ask) else {
        return None;
    };
    let picks = s.cloud_picks.as_ref()?;
    let n_items = picks.items.len();
    Some(sheet(
        p,
        Sheet {
            mode: Mode::Cloud,
            title: tf!("Sync {}", "ซิงก์ {}", project),
            subtitle: tf!(
                "{} between this computer and the cloud. Pick the value each one keeps.",
                "มี {} ระหว่างเครื่องนี้และคลาวด์ เลือกค่าที่จะเก็บของแต่ละตัว",
                count(n_items, "difference", "differences", "จุดที่ต่าง")
            ),
            reveal: picks.reveal,
            list: conflict_list(p, Mode::Cloud, &picks.items, picks.reveal),
            bulk: [
                button(
                    p,
                    "cloud-all-local",
                    t("This computer", "เครื่องนี้"),
                    Some(&ic::FILE),
                    Kind::Secondary,
                    true,
                    |s, _| pick_all_cloud(s, Pick::Master),
                ),
                button(
                    p,
                    "cloud-all-cloud",
                    t("Cloud", "คลาวด์"),
                    Some(&ic::CLOUD),
                    Kind::Secondary,
                    true,
                    |s, _| pick_all_cloud(s, Pick::Env),
                ),
            ],
            outcome: tf!(
                "Keyman writes your choices to the master file. Cancel changes nothing for {}.",
                "Keyman จะเขียนค่าที่เลือกลงในไฟล์หลัก ถ้ายกเลิก {} จะไม่เปลี่ยน",
                project
            ),
            buttons: [
                button(
                    p,
                    "cloud-resolve-cancel",
                    t("Cancel", "ยกเลิก"),
                    None,
                    Kind::Ghost,
                    true,
                    |s, _| s.cloud_resolve_cancel(),
                ),
                button(
                    p,
                    "cloud-resolve-apply",
                    t("Apply", "ใช้ค่าที่เลือก"),
                    Some(&ic::MERGE),
                    Kind::Primary,
                    true,
                    |s, _| s.cloud_resolve_apply(),
                ),
            ],
        },
    ))
}
