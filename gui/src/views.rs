//! Every screen of the app, from the custom title bar down.

use keyman_cli::save::Edit;
use keyman_cli::ui::display_path;
use rune::{Color, Cx, El, ScrollSpec, col, el, icon, row, text, text_input};

use crate::cloud_views;
use crate::core::{self, DiffLine, SavePreview};
use crate::i18n::{self, count, t};
use crate::icons as ic;
use crate::settings::{CLIPBOARD_TIMES, LANGUAGES, SIZES, Theme, WINDOW_SIZES};
use crate::widgets::*;
use crate::{App, Dialog, HelpPage, MenuAt, Panel, SettingsPage};

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    i18n::set(s.settings.language);
    let dark = match s.settings.theme {
        Theme::System => cx.system_dark(),
        Theme::Light => false,
        Theme::Dark => true,
    };
    let p = Paint::new(dark, s.settings.accent);
    let zoom = s.settings.zoom;
    cx.zoom(zoom);
    cx.titlebar_height(TITLE_H * zoom);
    cx.caption_color(p.text);
    cx.foreground(p.text);
    cx.focus_ring(2.0, 2.0, p.accent.alpha(0.6));

    let body = match &s.master {
        Ok(master) => {
            let mut page = match &s.selected {
                Some(name) if master.project(name).is_some() => project_page(s, p, name),
                Some(name) => new_project_page(s, p, name),
                None => empty_page(p),
            };
            if cloud_views::show_first_use(
                true,
                &s.cloud,
                s.settings.cloud_prompt_done,
                s.dialog.is_some() || s.question.is_some(),
            ) {
                page = col()
                    .key("with-first-use")
                    .w_full()
                    .gap(20.0)
                    .child(cloud_views::first_use_card(s, p))
                    .child(page);
            }
            row()
                .key("body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(sidebar(s, p))
                .child(content(p, page))
        }
        Err(error) => row()
            .key("body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(content(p, welcome_page(p, error))),
    };

    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(p.bg)
        .child(titlebar(s, p, cx))
        .child(body);
    if let Some(dialog) = s.dialog {
        root = root.child(dialog_view(s, p, dialog));
    }
    // A cloud task's question, above any dialog (the Account page under it, say).
    if let Some(sheet) = cloud_views::question_dialog(s, p) {
        root = root.child(
            scrim(p, "question-scrim").child(sheet.modal(false, |s: &mut App| s.cloud_cancel())),
        );
    }
    // Above the dialogs, so a notice from a cloud task (a wrong password) is not dimmed.
    if let Some(toast) = &s.toast {
        root = root.child(toast_view(p, toast));
    }
    if let Some(menu) = &s.menu {
        // Last frame's layout: the window's size, and the ⋯ button the panel drops from.
        let window = cx.layout("root").map_or((1100.0, 740.0), |r| (r[2], r[3]));
        let at = match &menu.at {
            MenuAt::Point(x, y) => (*x, *y),
            MenuAt::Below(key) => cx
                .layout(key)
                .map_or((window.0 / 2.0, window.1 / 3.0), |r| {
                    (r[0] + r[2] - crate::menu::MENU_W, r[1] + r[3] + 4.0)
                }),
        };
        root = root.child(crate::menu::folder_menu(s, p, at, window));
    }
    root
}

/// The scrolling main area.
fn content(p: Paint, page: El) -> El {
    col()
        .key("content")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .fill(p.bg)
        .child(
            col()
                .key("content-scroll")
                .w_full()
                .px(32.0)
                .pt(26.0)
                .pb(40.0)
                .child(page)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .shrink()
                .min_h(0.0),
        )
}

/// A tooltip with its shortcut: "Reload (⌘R)" on macOS, "Reload (Ctrl+R)" elsewhere.
fn tip(label: &str, key: char) -> String {
    let key = key.to_ascii_uppercase();
    if cfg!(target_os = "macos") {
        format!("{label} (⌘{key})")
    } else {
        format!("{label} (Ctrl+{key})")
    }
}

// ── Title bar ───────────────────────────────────────────────────────────────

fn titlebar(s: &App, p: Paint, cx: &mut Cx<App>) -> El {
    let insets = cx.chrome_insets();
    let mut crumbs = row().key("crumbs").gap(6.0).center_y().min_w(0.0).shrink();
    if let Ok(master) = &s.master {
        let file = master
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        crumbs = crumbs
            .child(icon(&ic::CHEVRON_RIGHT, 14.0).color(p.faint))
            .child(text(file).font(BODY).color(p.muted).truncate());
        if let Some(name) = &s.selected {
            crumbs = crumbs
                .child(icon(&ic::CHEVRON_RIGHT, 14.0).color(p.faint))
                .child(
                    text(name.clone())
                        .font(BODY_STRONG)
                        .color(p.text)
                        .truncate(),
                );
        }
    }
    row()
        .key("titlebar")
        .w_full()
        .h(TITLE_H)
        .fill(p.panel)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .center_y()
        .window_drag()
        .pl(insets.leading + 14.0)
        .pr(insets.trailing + 10.0)
        .gap(10.0)
        .child(
            el().key("logo")
                .size(24.0, 24.0)
                .center()
                .radius(7.0)
                .fill(p.accent)
                .child(icon(&ic::KEY, 15.0).color(p.on_accent).a11y_hidden()),
        )
        .child(text("Keyman").font(H2).color(p.text))
        .child(crumbs)
        .child(el().key("titlebar-spacer").grow(1.0))
        .children(s.master.is_ok().then(|| {
            icon_button(
                p,
                "tb-reload",
                &ic::REFRESH,
                &tip(t("Reload master file", "โหลดไฟล์หลักใหม่"), 'R'),
                |s, _| s.reload_and_report(),
            )
        }))
        .child(icon_button(
            p,
            "tb-fullscreen",
            &ic::EXPAND,
            if cfg!(target_os = "macos") {
                t("Full screen (⌃⌘F)", "เต็มหน้าจอ (⌃⌘F)")
            } else {
                t("Full screen (F11)", "เต็มหน้าจอ (F11)")
            },
            |_, tx| tx.toggle_full_screen(),
        ))
        .child(cloud_views::cloud_button(s, p))
        .child(icon_button(
            p,
            "tb-help",
            &ic::HELP,
            t("Help (F1)", "ความช่วยเหลือ (F1)"),
            |s, _| s.dialog = Some(Dialog::Help(HelpPage::Start)),
        ))
        .child(icon_button(
            p,
            "tb-settings",
            &ic::GEAR,
            &tip(t("Settings", "การตั้งค่า"), ','),
            |s, _| s.dialog = Some(Dialog::Settings(SettingsPage::General)),
        ))
}

// ── Sidebar ─────────────────────────────────────────────────────────────────

fn sidebar(s: &App, p: Paint) -> El {
    let Ok(master) = &s.master else { return el() };
    let query = s.search.text.trim().to_lowercase();
    let search = row()
        .key("search-box")
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(8.0)
        .fill(if p.dark {
            Color::BLACK.alpha(0.2)
        } else {
            Color::WHITE
        })
        .border(1.0, p.border)
        .child(icon(&ic::SEARCH, 15.0).color(p.faint))
        .child(
            text_input(&s.search, BODY)
                .key("search")
                .label(t("Search projects", "ค้นหาโปรเจกต์"))
                .single_line()
                .placeholder(t("Search projects", "ค้นหาโปรเจกต์"), p.faint)
                .color(p.text)
                .on_edit(|s: &mut App| &mut s.search)
                .grow(1.0)
                .min_w(0.0),
        );

    let mut list = col().key("projects").w_full().gap(2.0);
    let mut shown = 0;
    for project in master
        .projects
        .iter()
        .filter(|project| query.is_empty() || project.name.to_lowercase().contains(&query))
    {
        shown += 1;
        let name = project.name.clone();
        let selected = s.selected.as_ref() == Some(&name);
        let (status, detail) = match &project.variables {
            Ok(vars) => (p.green, vars.len().to_string()),
            Err(_) => (p.red, "!".to_owned()),
        };
        list = list.child(
            row()
                .key(format!("project-{name}"))
                .role("button")
                .label(&name)
                .w_full()
                .h(32.0)
                .px(10.0)
                .gap(10.0)
                .center_y()
                .radius(7.0)
                .fill(if selected {
                    p.accent.alpha(if p.dark { 0.2 } else { 0.12 })
                } else {
                    Color::TRANSPARENT
                })
                .hover(if selected {
                    Color::TRANSPARENT
                } else {
                    p.hover
                })
                .press(p.press)
                .focusable(true)
                .on_press(move |s: &mut App| s.select(Some(name.clone())))
                .child(dot(status))
                .child(
                    text(project.name.clone())
                        .font(if selected { BODY_STRONG } else { BODY })
                        .color(p.text)
                        .truncate()
                        .grow(1.0)
                        .min_w(0.0)
                        .a11y_hidden(),
                )
                .child(
                    text(detail)
                        .font(MONO_SMALL)
                        .color(if selected { p.accent } else { p.faint })
                        .a11y_hidden(),
                ),
        );
    }
    if shown == 0 {
        let message = if master.projects.is_empty() {
            t("No projects yet", "ยังไม่มีโปรเจกต์")
        } else {
            t("No matching projects", "ไม่พบโปรเจกต์ที่ตรงกัน")
        };
        list = list.child(text(message).font(SMALL).color(p.faint).px(10.0).py(6.0));
    }

    let file = master
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let footer = row()
        .key("master-card")
        .w_full()
        .padding(10.0)
        .gap(10.0)
        .center_y()
        .radius(10.0)
        .fill(if p.dark {
            Color::BLACK.alpha(0.18)
        } else {
            Color::WHITE
        })
        .border(1.0, p.border)
        .child(icon_tile(&ic::FILE, 30.0, p.accent, p.dark))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(text(file).font(BODY_STRONG).color(p.text).truncate())
                .child(
                    text(display_path(&master.path))
                        .font(SMALL)
                        .color(p.faint)
                        .truncate(),
                ),
        )
        .child(icon_button(
            p,
            "edit-master",
            &ic::PENCIL,
            &tip(t("Edit master file", "แก้ไขไฟล์หลัก"), 'E'),
            |s, _| s.edit_master(),
        ))
        .child(icon_button(
            p,
            "load-master",
            &ic::FOLDER,
            &tip(t("Load another master file", "โหลดไฟล์หลักอื่น"), 'O'),
            crate::load_dialog,
        ));

    col()
        .key("sidebar")
        .w(264.0)
        .h_full()
        .px(12.0)
        .pt(14.0)
        .pb(12.0)
        .gap(10.0)
        .fill(p.panel)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.border)
        .child(search)
        .child(
            row()
                .key("projects-head")
                .w_full()
                .pl(10.0)
                .pt(6.0)
                .center_y()
                .child(text(t("PROJECTS", "โปรเจกต์")).font(CAPS).color(p.faint))
                .child(
                    text(format!("  {}", master.projects.len()))
                        .font(CAPS)
                        .color(p.faint),
                )
                .child(el().grow(1.0))
                .child(icon_button(
                    p,
                    "new-project",
                    &ic::PLUS,
                    &tip(t("New project from folder", "โปรเจกต์ใหม่จากโฟลเดอร์"), 'N'),
                    crate::new_project_dialog,
                )),
        )
        .child(
            list.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .shrink()
            .min_h(0.0),
        )
        .child(el().key("sidebar-spacer").grow(1.0))
        .child(footer)
}

// ── Pages ───────────────────────────────────────────────────────────────────

fn welcome_page(p: Paint, error: &str) -> El {
    let reason = error.lines().next().unwrap_or_default();
    let feature = |key: &str, glyph, title: &str, body: &str| {
        row()
            .key(key)
            .w_full()
            .gap(14.0)
            .child(icon_tile(glyph, 36.0, p.accent, p.dark))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(text(title.to_owned()).font(BODY_STRONG).color(p.text))
                    .child(text(body.to_owned()).font(SMALL).color(p.muted).wrap()),
            )
    };
    col()
        .key("welcome")
        .w_full()
        .pt(40.0)
        .center()
        .child(
            col()
                .key("welcome-card")
                .w(560.0)
                .gap(22.0)
                .child(
                    el().key("welcome-logo")
                        .size(56.0, 56.0)
                        .center()
                        .radius(16.0)
                        .fill(p.accent)
                        .shadow(6.0, 18.0, p.accent.alpha(0.35))
                        .child(icon(&ic::KEY, 30.0).color(p.on_accent)),
                )
                .child(
                    col()
                        .gap(8.0)
                        .child(text(t("Welcome to Keyman", "ยินดีต้อนรับสู่ Keyman")).font(H1).color(p.text))
                        .child(
                            text(t("Keep every project's secrets in one private YAML file, and write the right .env into any project folder.", "เก็บความลับของทุกโปรเจกต์ไว้ในไฟล์ YAML ส่วนตัวไฟล์เดียว แล้วเขียน .env ที่ถูกต้องลงในโฟลเดอร์โปรเจกต์ใดก็ได้"))
                                .font(BODY)
                                .color(p.muted)
                                .wrap(),
                        ),
                )
                .child(
                    col()
                        .gap(16.0)
                        .child(feature("f1", &ic::SHIELD, t("One private master file", "ไฟล์หลักส่วนตัวไฟล์เดียว"), t("Stored with owner-only permissions, never in Git.", "มีเพียงเจ้าของที่อ่านได้ และไม่ถูกเก็บใน Git")))
                        .child(feature("f2", &ic::DOWNLOAD, t("Write .env in one click", "เขียน .env ได้ในคลิกเดียว"), t("Pick a project folder and Keyman writes its variables.", "เลือกโฟลเดอร์โปรเจกต์ แล้ว Keyman จะเขียนตัวแปรให้")))
                        .child(feature("f3", &ic::COMPARE, t("Compare and save back", "เปรียบเทียบและบันทึกกลับ"), t("See what changed in .env and copy it into the master file.", "ดูว่า .env เปลี่ยนอะไรไป แล้วคัดลอกกลับไปยังไฟล์หลัก"))),
                )
                .child(
                    row()
                        .key("welcome-actions")
                        .gap(10.0)
                        .child(button(p, "init", t("Create ~/.keyman.yml", "สร้าง ~/.keyman.yml"), Some(&ic::PLUS), Kind::Primary, true, |s, _| s.init_master()))
                        .child(button(p, "load-existing", t("Load existing file…", "โหลดไฟล์ที่มีอยู่…"), Some(&ic::FOLDER), Kind::Secondary, true, crate::load_dialog)),
                )
                .children((!reason.is_empty()).then(|| {
                    row()
                        .gap(6.0)
                        .center_y()
                        .child(icon(&ic::INFO, 14.0).color(p.faint))
                        .child(text(format!("{reason}.")).font(SMALL).color(p.faint))
                })),
        )
}

fn empty_page(p: Paint) -> El {
    col()
        .key("empty")
        .w_full()
        .pt(80.0)
        .gap(12.0)
        .center()
        .child(icon_tile(&ic::LIST, 48.0, p.muted, p.dark))
        .child(
            text(t("No project selected", "ยังไม่ได้เลือกโปรเจกต์"))
                .font(H2)
                .color(p.text),
        )
        .child(
            text(t(
                "Edit the master file to add projects, or create one from a folder's .env.",
                "แก้ไขไฟล์หลักเพื่อเพิ่มโปรเจกต์ หรือสร้างจาก .env ของโฟลเดอร์",
            ))
            .font(BODY)
            .color(p.muted),
        )
        .child(
            row()
                .gap(10.0)
                .pt(6.0)
                .child(button(
                    p,
                    "empty-new",
                    t("New project from folder", "โปรเจกต์ใหม่จากโฟลเดอร์"),
                    Some(&ic::PLUS),
                    Kind::Primary,
                    true,
                    crate::new_project_dialog,
                ))
                .child(button(
                    p,
                    "empty-edit",
                    t("Edit master file", "แก้ไขไฟล์หลัก"),
                    Some(&ic::PENCIL),
                    Kind::Secondary,
                    true,
                    |s, _| s.edit_master(),
                )),
        )
}

fn page_header(p: Paint, name: &str, status: El, subtitle: String, actions: El) -> El {
    row()
        .key("page-header")
        .w_full()
        .gap(16.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(6.0)
                .child(
                    row()
                        .gap(10.0)
                        .center_y()
                        .child(
                            text(name.to_owned())
                                .font(H1)
                                .color(p.text)
                                .truncate()
                                .min_w(0.0),
                        )
                        .child(status),
                )
                .child(text(subtitle).font(BODY).color(p.muted).truncate()),
        )
        .child(actions)
}

fn project_page(s: &App, p: Paint, name: &str) -> El {
    let variables = s.variables().cloned().unwrap_or_else(|| Ok(Vec::new()));
    let folder_note = s
        .folder()
        .map(|f| format!(" · {}", display_path(&f)))
        .unwrap_or_default();
    let page = col().key(format!("page-{name}")).w_full().gap(20.0);
    let vars = match variables {
        Ok(vars) => vars,
        Err(problem) => {
            return page
                .child(page_header(
                    p,
                    name,
                    badge(p, t("Has a problem", "มีปัญหา"), p.red),
                    t(
                        "This project can't be used until it's fixed",
                        "ใช้โปรเจกต์นี้ไม่ได้จนกว่าจะแก้ไข",
                    )
                    .into(),
                    el(),
                ))
                .child(
                    card(p, "problem")
                        .padding(18.0)
                        .gap(12.0)
                        .child(
                            row()
                                .gap(10.0)
                                .center_y()
                                .child(icon(&ic::ALERT, 18.0).color(p.red))
                                .child(
                                    text(t("The master file breaks a rule here", "ไฟล์หลักผิดกฎตรงนี้"))
                                        .font(H2)
                                        .color(p.text),
                                ),
                        )
                        .child(
                            text(problem)
                                .font(MONO)
                                .color(p.text)
                                .wrap()
                                .selectable(true),
                        )
                        .child(
                            row()
                                .gap(10.0)
                                .child(button(
                                    p,
                                    "fix-edit",
                                    t("Edit master file", "แก้ไขไฟล์หลัก"),
                                    Some(&ic::PENCIL),
                                    Kind::Primary,
                                    true,
                                    |s, _| s.edit_master(),
                                ))
                                .child(button(
                                    p,
                                    "fix-reload",
                                    t("Reload", "โหลดใหม่"),
                                    Some(&ic::REFRESH),
                                    Kind::Secondary,
                                    true,
                                    |s, _| s.reload_and_report(),
                                )),
                        ),
                );
        }
    };

    let export = core::export_lines(&vars);
    let actions = row()
        .gap(8.0)
        .child(button(
            p,
            "reveal-all",
            if s.reveal_all {
                t("Hide values", "ซ่อนค่า")
            } else {
                t("Reveal values", "แสดงค่า")
            },
            Some(if s.reveal_all { &ic::EYE_OFF } else { &ic::EYE }),
            Kind::Secondary,
            !vars.is_empty(),
            |s, _| {
                s.reveal_all = !s.reveal_all;
                s.revealed.clear();
            },
        ))
        .child(button(
            p,
            "copy-export",
            t("Copy export", "คัดลอก export"),
            Some(&ic::COPY),
            Kind::Secondary,
            !vars.is_empty(),
            move |s, tx| {
                let secs = s.settings.clipboard_secs;
                let _ = tx.copy_secret(export.clone(), s.clipboard_time());
                s.ok(tf!(
                    "Copied export lines. The clipboard clears in {} seconds.",
                    "คัดลอกคำสั่ง export แล้ว คลิปบอร์ดจะถูกล้างใน {} วินาที",
                    secs
                ));
            },
        ));

    // Variables and folders side by side; both columns grow with the window.
    let columns = row()
        .key("columns")
        .w_full()
        .gap(20.0)
        .child(
            col()
                .key("col-vars")
                .grow(1.0)
                .min_w(0.0)
                .child(variables_tab(s, p, &vars)),
        )
        .child(
            col()
                .key("col-folders")
                .grow(1.0)
                .min_w(0.0)
                .child(folder_tab(s, p, true)),
        );
    page.child(page_header(
        p,
        name,
        badge(p, t("Valid", "ถูกต้อง"), p.green),
        format!(
            "{}{folder_note}",
            count(vars.len(), "variable", "variables", "ตัวแปร")
        ),
        actions,
    ))
    .child(columns)
}

fn new_project_page(s: &App, p: Paint, name: &str) -> El {
    col()
        .key(format!("page-new-{name}"))
        .w_full()
        .gap(20.0)
        .child(page_header(
            p,
            name,
            badge(p, t("New project", "โปรเจกต์ใหม่"), p.accent),
            t(
                "Save this folder's .env as a new project",
                "บันทึก .env ของโฟลเดอร์นี้เป็นโปรเจกต์ใหม่",
            )
            .into(),
            el(),
        ))
        .child(folder_tab(s, p, false))
}

// ── Variables ───────────────────────────────────────────────────────────────

const NAME_W: f32 = 140.0;

fn variables_tab(s: &App, p: Paint, vars: &[(String, String)]) -> El {
    let hidden = if s.reveal_all {
        t("values shown", "แสดงค่าอยู่")
    } else {
        t("values hidden", "ซ่อนค่าอยู่")
    };
    let mut table = card(p, "variables").child(
        row()
            .key("vars-title")
            .w_full()
            .px(18.0)
            .py(14.0)
            .gap(10.0)
            .center_y()
            .border_sides([0.0, 0.0, 1.0, 0.0], p.separator)
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(2.0)
                    .child(text(t("Variables", "ตัวแปร")).font(H2).color(p.text))
                    .child(
                        text(format!(
                            "{} · {hidden}",
                            count(vars.len(), "variable", "variables", "ตัวแปร")
                        ))
                        .font(SMALL)
                        .color(p.muted),
                    ),
            ),
    );
    if vars.is_empty() {
        return table.child(
            col()
                .key("vars-empty")
                .w_full()
                .padding(24.0)
                .gap(6.0)
                .center()
                .child(text(t("No variables yet", "ยังไม่มีตัวแปร")).font(BODY_STRONG).color(p.text))
                .child(
                    text(t("Add some with Edit master file, or save a folder's .env into this project.", "เพิ่มได้ด้วยการแก้ไขไฟล์หลัก หรือบันทึก .env ของโฟลเดอร์ลงในโปรเจกต์นี้"))
                        .font(SMALL)
                        .color(p.muted)
                        .wrap(),
                ),
        );
    }
    table = table.child(
        row()
            .key("vars-head")
            .w_full()
            .h(36.0)
            .px(18.0)
            .gap(16.0)
            .center_y()
            .border_sides([0.0, 0.0, 1.0, 0.0], p.separator)
            .child(text(t("NAME", "ชื่อ")).font(CAPS).color(p.faint).w(NAME_W))
            .child(text(t("VALUE", "ค่า")).font(CAPS).color(p.faint).grow(1.0)),
    );
    for (i, (name, value)) in vars.iter().enumerate() {
        let shown = s.reveal_all || s.revealed.contains(name);
        let (toggle_name, copy_value) = (name.clone(), value.clone());
        let mut r = row()
            .key(format!("var-{name}"))
            .w_full()
            .h(46.0)
            .pl(18.0)
            .pr(10.0)
            .gap(12.0)
            .center_y()
            .hover(p.hover)
            .child(
                text(name.clone())
                    .font(MONO)
                    .color(p.text)
                    .w(NAME_W)
                    .truncate()
                    .selectable(true),
            )
            .child(
                text(if shown {
                    value.clone()
                } else {
                    "••••••••••".to_owned()
                })
                .font(MONO)
                .color(if shown { p.text } else { p.faint })
                .truncate()
                .selectable(shown)
                .grow(1.0)
                .min_w(0.0),
            );
        if !s.reveal_all {
            r = r.child(icon_button(
                p,
                &format!("show-{name}"),
                if shown { &ic::EYE_OFF } else { &ic::EYE },
                if shown {
                    t("Hide value", "ซ่อนค่า")
                } else {
                    t("Show value", "แสดงค่า")
                },
                move |s, _| {
                    if !s.revealed.remove(&toggle_name) {
                        s.revealed.insert(toggle_name.clone());
                    }
                },
            ));
        }
        r = r.child(icon_button(
            p,
            &format!("copy-{name}"),
            &ic::COPY,
            t("Copy value", "คัดลอกค่า"),
            move |s, tx| {
                let secs = s.settings.clipboard_secs;
                let _ = tx.copy_secret(copy_value.clone(), s.clipboard_time());
                s.ok(tf!(
                    "Copied. The clipboard clears in {} seconds.",
                    "คัดลอกแล้ว คลิปบอร์ดจะถูกล้างใน {} วินาที",
                    secs
                ));
            },
        ));
        if i > 0 {
            table = table.child(
                el().key(format!("var-sep-{i}"))
                    .w_full()
                    .px(18.0)
                    .child(hairline(p, "line")),
            );
        }
        table = table.child(r);
    }
    table.pb(4.0)
}

// ── Folders ───────────────────────────────────────────────────────────

fn folder_tab(s: &App, p: Paint, exists: bool) -> El {
    if let (true, Some(name), Some(Ok(vars))) = (exists, &s.selected, s.variables()) {
        return crate::diagram::tie_diagram(s, p, name, vars);
    }
    // A new project: its one folder, and the step that saves its .env as the project.
    let folder = s.folder();
    let env_exists = folder.as_ref().is_some_and(|f| f.join(".env").is_file());
    let location = folder.as_ref().map_or_else(
        || t("No folder chosen", "ยังไม่ได้เลือกโฟลเดอร์").to_owned(),
        |f| display_path(f),
    );
    card(p, "folder")
        .padding(16.0)
        .gap(14.0)
        .child(
            row()
                .w_full()
                .gap(14.0)
                .center_y()
                .child(icon_tile(&ic::FOLDER, 40.0, p.accent, p.dark))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(4.0)
                        .child(
                            text(t("Project folder", "โฟลเดอร์โปรเจกต์"))
                                .font(SMALL)
                                .color(p.muted),
                        )
                        .child(text(location).font(MONO).color(p.text).truncate()),
                )
                .child(if env_exists {
                    badge(p, t(".env present", "มี .env"), p.green)
                } else {
                    badge(p, t("No .env yet", "ยังไม่มี .env"), p.yellow)
                }),
        )
        .child(
            row()
                .gap(8.0)
                .child(button(
                    p,
                    "create-project",
                    t("Create project from .env…", "สร้างโปรเจกต์จาก .env…"),
                    Some(&ic::UPLOAD),
                    Kind::Primary,
                    env_exists,
                    |s, _| s.preview_save(),
                ))
                .child(button(
                    p,
                    "choose-folder",
                    t("Choose another…", "เลือกโฟลเดอร์อื่น…"),
                    None,
                    Kind::Secondary,
                    true,
                    crate::new_project_dialog,
                )),
        )
}

fn panel_card(p: Paint, key: &str, glyph: &'static rune::Icon, title: &str, trailing: El) -> El {
    card(p, key).child(
        row()
            .key(format!("{key}-head"))
            .w_full()
            .h(48.0)
            .px(16.0)
            .gap(10.0)
            .center_y()
            .border_sides([0.0, 0.0, 1.0, 0.0], p.separator)
            .child(icon(glyph, 16.0).color(p.muted))
            .child(text(title.to_owned()).font(BODY_STRONG).color(p.text))
            .child(el().grow(1.0))
            .child(trailing),
    )
}

fn diff_view(s: &App, p: Paint, lines: &[DiffLine]) -> El {
    let reveal = s.reveal_all;
    let shown = |name: &str, value: &str| {
        if reveal {
            format!("{name}=\"{value}\"")
        } else {
            format!("{name}=••••••••")
        }
    };
    let count = |edit: Edit| lines.iter().filter(|line| line.edit == edit).count();
    let summary: Vec<String> = [
        (count(Edit::Changed), t("changed", "เปลี่ยนค่า")),
        (count(Edit::Added), t("only in .env", "มีเฉพาะใน .env")),
        (count(Edit::Removed), t("only in master", "มีเฉพาะในไฟล์หลัก")),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, label)| tf!("{} {}", "{1} {0}", n, label))
    .collect();
    let same = summary.is_empty();

    let line = |key: String, sign: &str, body: String, tint: Color, bg: Color| {
        row()
            .key(key)
            .w_full()
            .px(16.0)
            .py(3.0)
            .fill(bg)
            .child(text(sign.to_owned()).font(MONO).color(tint).w(18.0))
            .child(
                text(body)
                    .font(MONO)
                    .color(tint)
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            )
    };
    let mut out = col().key("diff-lines").w_full().py(8.0);
    for l in lines {
        let key = format!("diff-{}", l.name);
        out = match l.edit {
            Edit::Unchanged | Edit::Kept => out.child(line(
                key,
                " ",
                shown(&l.name, &l.master),
                p.muted,
                Color::TRANSPARENT,
            )),
            Edit::Changed => out
                .child(line(
                    format!("{key}-old"),
                    "−",
                    shown(&l.name, &l.master),
                    p.red,
                    p.red.alpha(0.08),
                ))
                .child(line(
                    format!("{key}-new"),
                    "+",
                    shown(&l.name, &l.env),
                    p.green,
                    p.green.alpha(0.08),
                )),
            Edit::Removed => out.child(line(
                key,
                "−",
                shown(&l.name, &l.master),
                p.red,
                p.red.alpha(0.08),
            )),
            Edit::Added => out.child(line(
                key,
                "+",
                shown(&l.name, &l.env),
                p.green,
                p.green.alpha(0.08),
            )),
        };
    }
    let status = if same {
        badge(p, t("In sync", "ตรงกัน"), p.green)
    } else {
        badge(p, &summary.join(" · "), p.yellow)
    };
    panel_card(
        p,
        "diff",
        &ic::COMPARE,
        t("master file → .env", "ไฟล์หลัก → .env"),
        row().gap(6.0).center_y().child(status).child(icon_button(
            p,
            "diff-close",
            &ic::CLOSE,
            t("Close", "ปิด"),
            |s, _| s.close_panel(),
        )),
    )
    .child(out)
    .child(
        row()
            .key("diff-foot")
            .w_full()
            .px(16.0)
            .py(10.0)
            .gap(8.0)
            .center_y()
            .border_sides([1.0, 0.0, 0.0, 0.0], p.separator)
            .child(
                text(if reveal {
                    t(
                        "Red is the master file, green is .env.",
                        "สีแดงคือไฟล์หลัก สีเขียวคือ .env",
                    )
                } else {
                    t(
                        "Red is the master file, green is .env. Reveal values to see them.",
                        "สีแดงคือไฟล์หลัก สีเขียวคือ .env กดแสดงค่าเพื่อดูค่า",
                    )
                })
                .font(SMALL)
                .color(p.muted)
                .grow(1.0),
            )
            .children((!same).then(|| {
                button(
                    p,
                    "diff-save",
                    t("Save to master", "บันทึกลงไฟล์หลัก"),
                    Some(&ic::UPLOAD),
                    Kind::Secondary,
                    true,
                    |s, _| s.preview_save(),
                )
            }))
            .children((!same).then(|| {
                button(
                    p,
                    "diff-inject",
                    t("Overwrite .env", "เขียนทับ .env"),
                    Some(&ic::DOWNLOAD),
                    Kind::Secondary,
                    true,
                    |s, _| s.inject(),
                )
            })),
    )
}

fn save_view(s: &App, p: Paint, preview: &SavePreview) -> El {
    let plan = &preview.plan;
    let close = icon_button(
        p,
        "save-close",
        &ic::CLOSE,
        t("Close", "ปิด"),
        |s, _| s.close_panel(),
    );
    if preview.exists && plan.changes() == 0 {
        return panel_card(
            p,
            "save",
            &ic::UPLOAD,
            t("Save .env to master", "บันทึก .env ลงไฟล์หลัก"),
            close,
        )
        .child(
            row()
                .padding(16.0)
                .gap(10.0)
                .center_y()
                .child(icon(&ic::CHECK_CIRCLE, 18.0).color(p.green))
                .child(
                    text(t(
                        "The master file already matches .env. Nothing to save.",
                        "ไฟล์หลักตรงกับ .env อยู่แล้ว ไม่มีอะไรต้องบันทึก",
                    ))
                    .font(BODY)
                    .color(p.text),
                ),
        );
    }
    let mut rows = col().key("save-rows").w_full().py(8.0);
    for (name, edit) in &plan.edits {
        let (sign, tint, note) = match edit {
            Edit::Added => ("+", p.green, "added"),
            Edit::Changed => ("~", p.yellow, "changed"),
            Edit::Removed => ("−", p.red, "removed"),
            Edit::Kept => (
                "=",
                p.faint,
                t("only in master, kept", "มีเฉพาะในไฟล์หลัก เก็บไว้"),
            ),
            Edit::Unchanged => continue,
        };
        rows = rows.child(
            row()
                .key(format!("save-{name}"))
                .w_full()
                .px(16.0)
                .h(30.0)
                .gap(10.0)
                .center_y()
                .child(text(sign).font(MONO).color(tint).w(14.0))
                .child(
                    text(name.clone())
                        .font(MONO)
                        .color(p.text)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(badge(p, note, tint)),
        );
    }
    let unchanged = plan.count(Edit::Unchanged);
    let changes = plan.changes();
    let title = if preview.exists {
        t("Save .env to master", "บันทึก .env ลงไฟล์หลัก")
    } else {
        t("Create project from .env", "สร้างโปรเจกต์จาก .env")
    };
    let mut foot = row()
        .key("save-foot")
        .w_full()
        .px(16.0)
        .py(12.0)
        .gap(12.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], p.separator);
    if preview.exists {
        foot = foot.child(
            row()
                .key("replace-row")
                .gap(10.0)
                .center_y()
                .child(switch(
                    p,
                    "replace",
                    t("Remove variables only in master", "ลบตัวแปรที่มีเฉพาะในไฟล์หลัก"),
                    s.replace,
                    |s| {
                        s.replace = !s.replace;
                        s.preview_save();
                    },
                ))
                .child(
                    text(t(
                        "Remove variables only in master",
                        "ลบตัวแปรที่มีเฉพาะในไฟล์หลัก",
                    ))
                    .font(SMALL)
                    .color(p.muted),
                ),
        );
    }
    foot = foot
        .child(el().grow(1.0))
        .child(button(
            p,
            "save-cancel",
            t("Cancel", "ยกเลิก"),
            None,
            Kind::Ghost,
            true,
            |s, _| s.close_panel(),
        ))
        .child(button(
            p,
            "save-apply",
            &if preview.exists {
                tf!(
                    "Save {}",
                    "บันทึก {}",
                    count(changes, "change", "changes", "รายการ")
                )
            } else {
                t("Create project", "สร้างโปรเจกต์").to_owned()
            },
            Some(&ic::UPLOAD),
            Kind::Primary,
            changes > 0,
            |s, _| s.apply_save(),
        ));
    panel_card(p, "save", &ic::UPLOAD, title, close)
        .child(rows)
        .children((unchanged > 0).then(|| {
            text(tf!(
                "{} unchanged",
                "ไม่เปลี่ยน {}",
                count(unchanged, "variable", "variables", "ตัวแปร")
            ))
            .font(SMALL)
            .color(p.faint)
            .px(16.0)
            .pb(10.0)
        }))
        .child(foot)
}

// ── Toast ───────────────────────────────────────────────────────────────────

fn toast_view(p: Paint, toast: &crate::Toast) -> El {
    let tint = if toast.ok { p.green } else { p.red };
    row()
        .key(format!("toast-{}", toast.id))
        .pin(None, None, Some(20.0), Some(20.0))
        .max_w(460.0)
        .padding(12.0)
        .gap(10.0)
        .radius(10.0)
        .fill(p.raised)
        .border(1.0, p.border)
        .shadow(8.0, 24.0, p.shadow)
        .role("status")
        .child(
            icon(
                if toast.ok {
                    &ic::CHECK_CIRCLE
                } else {
                    &ic::ALERT
                },
                18.0,
            )
            .color(tint),
        )
        .child(
            text(toast.text.clone())
                .font(BODY)
                .color(p.text)
                .wrap()
                .selectable(true)
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            el().key("toast-close")
                .role("button")
                .label(t("Dismiss", "ปิด"))
                .size(20.0, 20.0)
                .center()
                .radius(5.0)
                .hover(p.hover)
                .focusable(true)
                .on_press(|s: &mut App| s.toast = None)
                .child(icon(&ic::CLOSE, 13.0).color(p.faint)),
        )
}

// ── Dialogs ─────────────────────────────────────────────────────────────────

fn dialog_view(s: &App, p: Paint, dialog: Dialog) -> El {
    let sheet = match dialog {
        Dialog::Settings(page) => settings_dialog(s, p, page),
        Dialog::Help(page) => help_dialog(p, page),
        Dialog::ConfirmOverwrite => confirm_dialog(s, p),
        Dialog::ConfirmWriteAll => confirm_all_dialog(s, p),
        Dialog::ConfirmSignOut => cloud_views::confirm_sign_out(p),
        Dialog::Resolve => crate::resolve::resolve_dialog(s, p),
        Dialog::Panel => panel_dialog(s, p),
    };
    scrim(p, "scrim").child(sheet.modal(true, |s: &mut App| {
        s.dialog = None;
        s.panel = Panel::None;
    }))
}

/// The dimmed layer under a modal sheet.
fn scrim(p: Paint, key: &str) -> El {
    el().key(key)
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(Color::BLACK.alpha(if p.dark { 0.5 } else { 0.28 }))
        .center()
}

/// Compare or save-to-master, as a modal over the page.
fn panel_dialog(s: &App, p: Paint) -> El {
    let body = match &s.panel {
        Panel::Diff(lines) => diff_view(s, p, lines),
        Panel::Save(preview) => save_view(s, p, preview),
        Panel::None => el(),
    };
    col()
        .key("panel-sheet")
        .role("dialog")
        .w(680.0)
        .max_h(640.0)
        .radius(14.0)
        .fill(p.bg)
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
        .clip_rounded(13.0)
        .child(
            col()
                .key("panel-scroll")
                .w_full()
                .child(body)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .shrink()
                .min_h(0.0),
        )
}

/// A two-pane dialog: navigation on the left, the page on the right.
fn nav_dialog(
    p: Paint,
    key: &str,
    title: &str,
    nav: Vec<(&'static rune::Icon, &'static str, bool, Dialog)>,
    page_title: &str,
    page: El,
) -> El {
    let mut items = col().key("nav-items").w_full().gap(2.0);
    for (glyph, label, selected, target) in nav {
        items = items.child(
            row()
                .key(format!("nav-{label}"))
                .role("tab")
                .label(label)
                .checked(selected)
                .focusable(true)
                .w_full()
                .h(32.0)
                .px(10.0)
                .gap(10.0)
                .center_y()
                .radius(7.0)
                .fill(if selected {
                    p.accent.alpha(if p.dark { 0.2 } else { 0.12 })
                } else {
                    Color::TRANSPARENT
                })
                .hover(if selected {
                    Color::TRANSPARENT
                } else {
                    p.hover
                })
                .on_press(move |s: &mut App| s.dialog = Some(target))
                .child(icon(glyph, 16.0).color(if selected { p.accent } else { p.muted }))
                .child(
                    text(label)
                        .font(if selected { BODY_STRONG } else { BODY })
                        .color(p.text),
                ),
        );
    }
    row()
        .key(key)
        .role("dialog")
        .label(title)
        .w(780.0)
        .h(540.0)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.border)
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
        .clip_rounded(14.0)
        .child(
            col()
                .key("nav")
                .w(210.0)
                .h_full()
                .padding(14.0)
                .gap(12.0)
                .fill(p.panel)
                .border_sides([0.0, 1.0, 0.0, 0.0], p.border)
                .child(
                    text(title.to_owned())
                        .font(H2)
                        .color(p.text)
                        .px(8.0)
                        .pt(6.0)
                        .pb(4.0),
                )
                .child(items),
        )
        .child(
            col()
                .key("page")
                .grow(1.0)
                .h_full()
                .min_w(0.0)
                .child(
                    row()
                        .key("page-head")
                        .w_full()
                        .h(56.0)
                        .px(24.0)
                        .center_y()
                        .child(text(page_title.to_owned()).font(H2).color(p.text).grow(1.0))
                        .child(icon_button(
                            p,
                            "dialog-close",
                            &ic::CLOSE,
                            t("Close (Esc)", "ปิด (Esc)"),
                            |s, _| s.dialog = None,
                        )),
                )
                .child(
                    col()
                        .key("page-scroll")
                        .w_full()
                        .px(24.0)
                        .pb(24.0)
                        .gap(22.0)
                        .child(page)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .shrink()
                        .min_h(0.0),
                ),
        )
}

fn settings_dialog(s: &App, p: Paint, page: SettingsPage) -> El {
    use SettingsPage::*;
    let nav = vec![
        (
            &ic::SLIDERS,
            t("General", "ทั่วไป"),
            page == General,
            Dialog::Settings(General),
        ),
        (
            &ic::PALETTE,
            t("Appearance", "รูปลักษณ์"),
            page == Appearance,
            Dialog::Settings(Appearance),
        ),
        (
            &ic::SHIELD,
            t("Privacy", "ความเป็นส่วนตัว"),
            page == Privacy,
            Dialog::Settings(Privacy),
        ),
        (
            &ic::CLOUD,
            t("Account", "บัญชี"),
            page == Account,
            Dialog::Settings(Account),
        ),
        (
            &ic::INFO,
            t("About", "เกี่ยวกับ"),
            page == About,
            Dialog::Settings(About),
        ),
    ];
    let st = &s.settings;
    let (title, body) = match page {
        General => {
            let mut sizes = row().key("window-sizes").gap(8.0);
            for ((w, h), en, th) in WINDOW_SIZES {
                sizes = sizes.child(button(
                    p,
                    &format!("size-{en}"),
                    t(en, th),
                    None,
                    Kind::Secondary,
                    true,
                    move |_, tx| {
                        tx.resize_window((w, h));
                        tx.center_window();
                    },
                ));
            }
            let body = col()
                .w_full()
                .gap(22.0)
                .child(settings_group(
                    p,
                    "g-window",
                    t("WINDOW", "หน้าต่าง"),
                    vec![setting_row(
                        p,
                        "window-size",
                        t("Window size", "ขนาดหน้าต่าง"),
                        t(
                            "Resize and center the window.",
                            "ปรับขนาดและจัดหน้าต่างไว้กึ่งกลาง",
                        ),
                        sizes,
                    )],
                ))
                .child(settings_group(
                    p,
                    "g-folders",
                    t("PROJECT FOLDERS", "โฟลเดอร์โปรเจกต์"),
                    vec![
                        setting_row(
                            p,
                            "remember",
                            t("Remember project folders", "จำโฟลเดอร์โปรเจกต์"),
                            t(
                                "Keep each project's folder between launches.",
                                "จำโฟลเดอร์ของแต่ละโปรเจกต์ไว้เมื่อเปิดแอปครั้งถัดไป",
                            ),
                            switch(
                                p,
                                "remember-switch",
                                t("Remember project folders", "จำโฟลเดอร์โปรเจกต์"),
                                st.remember_folders,
                                |s| {
                                    s.update_settings(|st| {
                                        st.remember_folders = !st.remember_folders
                                    })
                                },
                            ),
                        ),
                        setting_row(
                            p,
                            "confirm",
                            t("Ask before overwriting .env", "ถามก่อนเขียนทับ .env"),
                            t(
                                "Confirm when Write .env would replace a different file.",
                                "ยืนยันก่อนเมื่อการเขียน .env จะแทนที่ไฟล์ที่ต่างกัน",
                            ),
                            switch(
                                p,
                                "confirm-switch",
                                t("Ask before overwriting .env", "ถามก่อนเขียนทับ .env"),
                                st.confirm_overwrite,
                                |s| {
                                    s.update_settings(|st| {
                                        st.confirm_overwrite = !st.confirm_overwrite
                                    })
                                },
                            ),
                        ),
                        setting_row(
                            p,
                            "gitwarn",
                            t(
                                "Warn when Git would commit .env",
                                "เตือนเมื่อ Git อาจ commit ไฟล์ .env",
                            ),
                            t(
                                "Check the folder's .gitignore after writing .env.",
                                "ตรวจ .gitignore ของโฟลเดอร์หลังเขียน .env",
                            ),
                            switch(
                                p,
                                "gitwarn-switch",
                                t(
                                    "Warn when Git would commit .env",
                                    "เตือนเมื่อ Git อาจ commit ไฟล์ .env",
                                ),
                                st.git_warning,
                                |s| s.update_settings(|st| st.git_warning = !st.git_warning),
                            ),
                        ),
                    ],
                ));
            (t("General", "ทั่วไป"), body)
        }
        Appearance => {
            let body = col().w_full().gap(22.0).child(settings_group(
                p,
                "g-look",
                t("LOOK", "การแสดงผล"),
                vec![
                    setting_row(
                        p,
                        "language",
                        t("Language", "ภาษา"),
                        t(
                            "Follow the system, or choose English or Thai.",
                            "ใช้ตามระบบ หรือเลือกภาษาอังกฤษหรือไทย",
                        ),
                        segmented(p, "language-seg", &LANGUAGES, st.language, |s, l| {
                            s.update_settings(|st| st.language = l)
                        }),
                    ),
                    setting_row(
                        p,
                        "theme",
                        t("Theme", "ธีม"),
                        t(
                            "Follow the system, or always use light or dark.",
                            "ใช้ตามระบบ หรือเลือกสว่างหรือมืดเสมอ",
                        ),
                        segmented(p, "theme-seg", &Theme::ALL, st.theme, |s, t| {
                            s.update_settings(|st| st.theme = t)
                        }),
                    ),
                    setting_row(
                        p,
                        "accent",
                        t("Accent colour", "สีเน้น"),
                        "",
                        swatches(p, st.accent, |s, i| s.update_settings(|st| st.accent = i)),
                    ),
                    setting_row(
                        p,
                        "size",
                        t("Interface size", "ขนาดอินเทอร์เฟซ"),
                        t("Scale text and controls.", "ปรับขนาดตัวอักษรและปุ่มต่าง ๆ"),
                        segmented(p, "size-seg", &SIZES, st.zoom, |s, z| {
                            s.update_settings(|st| st.zoom = z)
                        }),
                    ),
                ],
            ));
            (t("Appearance", "รูปลักษณ์"), body)
        }
        Privacy => {
            let body = col()
                .w_full()
                .gap(22.0)
                .child(settings_group(
                    p,
                    "g-clip",
                    t("CLIPBOARD", "คลิปบอร์ด"),
                    vec![setting_row(
                        p,
                        "clip",
                        t("Clear copied secrets after", "ล้างความลับที่คัดลอกหลังจาก"),
                        t("Copies are kept out of clipboard history, then cleared.", "สิ่งที่คัดลอกจะไม่อยู่ในประวัติคลิปบอร์ด และจะถูกล้างภายหลัง"),
                        segmented(p, "clip-seg", &CLIPBOARD_TIMES, st.clipboard_secs, |s, t| s.update_settings(|st| st.clipboard_secs = t)),
                    )],
                ))
                .child(settings_group(
                    p,
                    "g-values",
                    t("VALUES", "ค่า"),
                    vec![setting_row(
                        p,
                        "rehide",
                        t("Hide values when switching projects", "ซ่อนค่าเมื่อสลับโปรเจกต์"),
                        t("Revealed values are masked again when you open another project.", "ค่าที่แสดงอยู่จะถูกซ่อนอีกครั้งเมื่อเปิดโปรเจกต์อื่น"),
                        switch(p, "rehide-switch", t("Hide values when switching projects", "ซ่อนค่าเมื่อสลับโปรเจกต์"), st.rehide_on_switch, |s| {
                            s.update_settings(|st| st.rehide_on_switch = !st.rehide_on_switch)
                        }),
                    )],
                ))
                .child(
                    row()
                        .gap(8.0)
                        .child(icon(&ic::SHIELD, 14.0).color(p.faint))
                        .child(text(t("The master file and every .env Keyman writes are readable only by you (600) on macOS.", "บน macOS ไฟล์หลักและ .env ทุกไฟล์ที่ Keyman เขียนจะอ่านได้เฉพาะคุณ (600)")).font(SMALL).color(p.muted).wrap().grow(1.0).min_w(0.0)),
                );
            (t("Privacy", "ความเป็นส่วนตัว"), body)
        }
        Account => (t("Account", "บัญชี"), cloud_views::account_page(s, p)),
        About => {
            let master_path = s.master.as_ref().ok().map(|m| m.path.clone());
            let info = |key: &str, label: &str, value: String| {
                row()
                    .key(key)
                    .w_full()
                    .py(10.0)
                    .gap(12.0)
                    .center_y()
                    .child(text(label.to_owned()).font(BODY).color(p.muted).w(130.0))
                    .child(
                        text(value)
                            .font(MONO)
                            .color(p.text)
                            .truncate()
                            .grow(1.0)
                            .min_w(0.0)
                            .selectable(true),
                    )
            };
            let body = col()
                .w_full()
                .gap(22.0)
                .child(
                    row()
                        .gap(16.0)
                        .center_y()
                        .child(
                            el().size(56.0, 56.0)
                                .center()
                                .radius(16.0)
                                .fill(p.accent)
                                .child(icon(&ic::KEY, 30.0).color(p.on_accent)),
                        )
                        .child(
                            col()
                                .gap(4.0)
                                .child(text("Keyman").font(H1).color(p.text))
                                .child(
                                    text(tf!(
                                        "Desktop app {}",
                                        "แอปเดสก์ท็อป {}",
                                        env!("CARGO_PKG_VERSION")
                                    ))
                                    .font(BODY)
                                    .color(p.muted),
                                ),
                        ),
                )
                .child(settings_group(
                    p,
                    "g-about",
                    t("DETAILS", "รายละเอียด"),
                    vec![
                        info(
                            "about-master",
                            t("Master file", "ไฟล์หลัก"),
                            master_path.as_ref().map_or_else(
                                || t("Not loaded", "ยังไม่ได้โหลด").into(),
                                |p| display_path(p),
                            ),
                        ),
                        info(
                            "about-platform",
                            t("Platform", "แพลตฟอร์ม"),
                            format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH),
                        ),
                        info("about-ui", t("Built with", "สร้างด้วย"), "Rust · Rune".into()),
                    ],
                ))
                .child(
                    row()
                        .gap(8.0)
                        .child(button(
                            p,
                            "about-docs",
                            t("Documentation", "เอกสาร"),
                            Some(&ic::BOOK),
                            Kind::Secondary,
                            true,
                            |s, tx| crate::open_link(s, tx, crate::DOCS_URL),
                        ))
                        .child(button(
                            p,
                            "about-repo",
                            t("Source code", "ซอร์สโค้ด"),
                            Some(&ic::EXTERNAL),
                            Kind::Secondary,
                            true,
                            |s, tx| crate::open_link(s, tx, crate::REPO_URL),
                        ))
                        .children(master_path.map(|path| {
                            button(
                                p,
                                "about-reveal",
                                t("Show master file", "แสดงไฟล์หลัก"),
                                Some(&ic::FOLDER),
                                Kind::Secondary,
                                true,
                                move |s, _| {
                                    if let Err(error) = core::reveal_in_file_manager(&path) {
                                        s.fail(error);
                                    }
                                },
                            )
                        })),
                );
            (t("About", "เกี่ยวกับ"), body)
        }
    };
    nav_dialog(p, "settings", t("Settings", "การตั้งค่า"), nav, title, body)
}

fn help_dialog(p: Paint, page: HelpPage) -> El {
    use HelpPage::*;
    let nav = vec![
        (
            &ic::BOOK,
            t("Getting started", "เริ่มต้นใช้งาน"),
            page == Start,
            Dialog::Help(Start),
        ),
        (
            &ic::KEYBOARD,
            t("Shortcuts", "คีย์ลัด"),
            page == Shortcuts,
            Dialog::Help(Shortcuts),
        ),
        (
            &ic::TERMINAL,
            t("CLI commands", "คำสั่ง CLI"),
            page == Commands,
            Dialog::Help(Commands),
        ),
    ];
    let mod_key = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    let (title, body) = match page {
        Start => {
            let step = |n: usize, title: &str, body: &str| {
                row()
                    .key(format!("step-{n}"))
                    .w_full()
                    .gap(14.0)
                    .child(
                        el().size(28.0, 28.0)
                            .center()
                            .radius(14.0)
                            .fill(p.accent.alpha(0.15))
                            .child(text(n.to_string()).font(BODY_STRONG).color(p.accent)),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(3.0)
                            .child(text(title.to_owned()).font(BODY_STRONG).color(p.text))
                            .child(text(body.to_owned()).font(BODY).color(p.muted).wrap()),
                    )
            };
            let body = col()
                .w_full()
                .gap(18.0)
                .child(step(1, t("Create or load a master file", "สร้างหรือโหลดไฟล์หลัก"), t("One YAML file holds every project. Each top-level key is a project; each project lists its variables. Quote every value.", "ไฟล์ YAML ไฟล์เดียวเก็บทุกโปรเจกต์ คีย์ระดับบนสุดคือชื่อโปรเจกต์ และแต่ละโปรเจกต์มีรายการตัวแปร ใส่เครื่องหมายคำพูดครอบทุกค่า")))
                .child(step(2, t("Choose the project's folder", "ผูกโฟลเดอร์ของโปรเจกต์"), t("Tie the folders its code lives in: a checkout, a worktree, a deploy copy. Keyman syncs each one as you tie it.", "ผูกโฟลเดอร์ที่เก็บโค้ด เช่น checkout, worktree หรือชุดที่ใช้ deploy แล้ว Keyman จะซิงก์ให้ทันทีที่ผูก")))
                .child(step(3, t("Resolve differences", "แก้ไขความต่าง"), t("A folder with no .env gets one written. If its .env disagrees with the master file, pick the value each variable keeps and Keyman updates both.", "โฟลเดอร์ที่ยังไม่มี .env จะถูกเขียนให้ ถ้า .env ต่างจากไฟล์หลัก ให้เลือกค่าที่จะเก็บของแต่ละตัวแปร แล้ว Keyman จะอัปเดตทั้งสองไฟล์")))
                .child(step(4, t("Keep them in sync", "ทำให้ตรงกันอยู่เสมอ"), t("The diagram colours each folder by its status. Write to all refreshes every folder; Compare, Resolve and Save to master work on the active one.", "แผนภาพแสดงสีตามสถานะของแต่ละโฟลเดอร์ \"เขียนทั้งหมด\" จะอัปเดตทุกโฟลเดอร์ คลิกขวาที่โฟลเดอร์เพื่อเปรียบเทียบ แก้ไขความต่าง หรือบันทึกลงไฟล์หลัก")))
                .child(
                    row().pt(4.0).child(button(p, "help-docs", t("Read the full guide", "อ่านคู่มือฉบับเต็ม"), Some(&ic::EXTERNAL), Kind::Secondary, true, |s, tx| crate::open_link(s, tx, crate::DOCS_URL))),
                );
            (t("Getting started", "เริ่มต้นใช้งาน"), body)
        }
        Shortcuts => {
            let shortcut = |keys: Vec<String>, what: &str| {
                let mut caps = row().gap(4.0);
                for k in keys {
                    caps = caps.child(kbd(p, &k));
                }
                row()
                    .key(format!("sc-{what}"))
                    .w_full()
                    .py(9.0)
                    .center_y()
                    .child(text(what.to_owned()).font(BODY).color(p.text).grow(1.0))
                    .child(caps)
            };
            let m = |c: &str| vec![mod_key.to_owned(), c.to_owned()];
            let body = col()
                .w_full()
                .gap(22.0)
                .child(settings_group(
                    p,
                    "sc-app",
                    t("APP", "แอป"),
                    vec![
                        shortcut(m(","), t("Open settings", "เปิดการตั้งค่า")),
                        shortcut(vec!["F1".into()], t("Open help", "เปิดความช่วยเหลือ")),
                        shortcut(m("/"), t("Show shortcuts", "แสดงคีย์ลัด")),
                        shortcut(vec!["Esc".into()], t("Close a dialog", "ปิดหน้าต่างย่อย")),
                        shortcut(
                            if cfg!(target_os = "macos") {
                                vec!["⌃".into(), "⌘".into(), "F".into()]
                            } else {
                                vec!["F11".into()]
                            },
                            t("Toggle full screen", "สลับโหมดเต็มหน้าจอ"),
                        ),
                    ],
                ))
                .child(settings_group(
                    p,
                    "sc-master",
                    t("MASTER FILE", "ไฟล์หลัก"),
                    vec![
                        shortcut(m("R"), t("Reload and check", "โหลดใหม่และตรวจสอบ")),
                        shortcut(
                            m("E"),
                            t("Edit in your text editor", "แก้ไขด้วยโปรแกรมแก้ไขข้อความ"),
                        ),
                        shortcut(m("O"), t("Load another master file", "โหลดไฟล์หลักอื่น")),
                        shortcut(m("N"), t("New project from folder", "โปรเจกต์ใหม่จากโฟลเดอร์")),
                        shortcut(m("F"), t("Search projects", "ค้นหาโปรเจกต์")),
                    ],
                ));
            (t("Keyboard shortcuts", "คีย์ลัดแป้นพิมพ์"), body)
        }
        Commands => {
            let pair = |cli: &str, app: &str| {
                row()
                    .key(format!("cmd-{cli}"))
                    .w_full()
                    .py(9.0)
                    .gap(12.0)
                    .center_y()
                    .child(text(cli.to_owned()).font(MONO).color(p.accent).w(210.0))
                    .child(
                        text(app.to_owned())
                            .font(BODY)
                            .color(p.text)
                            .wrap()
                            .grow(1.0)
                            .min_w(0.0),
                    )
            };
            let body = col()
                .w_full()
                .gap(16.0)
                .child(text(t("The app and the keyman command share the same master file, so you can use both.", "แอปและคำสั่ง keyman ใช้ไฟล์หลักเดียวกัน จึงใช้ร่วมกันได้")).font(BODY).color(p.muted).wrap())
                .child(settings_group(
                    p,
                    "cmds",
                    t("CLI → APP", "CLI → แอป"),
                    vec![
                        pair("keyman init", t("Welcome › Create ~/.keyman.yml", "หน้าต้อนรับ › สร้าง ~/.keyman.yml")),
                        pair("keyman load <file>", t("Sidebar › Load another master file", "แถบข้าง › โหลดไฟล์หลักอื่น")),
                        pair("keyman edit", t("Sidebar › Edit master file", "แถบข้าง › แก้ไขไฟล์หลัก")),
                        pair("keyman status", t("Sidebar project list", "รายการโปรเจกต์ในแถบข้าง")),
                        pair("keyman show --reveal", t("Variables › Reveal values", "ตัวแปร › แสดงค่า")),
                        pair("keyman inject", t("Folders › Write .env (or Write to all)", "โฟลเดอร์ › เขียน .env (หรือเขียนทั้งหมด)")),
                        pair("keyman diff", t("Folders › Compare, or Resolve differences", "โฟลเดอร์ › เปรียบเทียบ หรือแก้ไขความต่าง")),
                        pair("keyman save", t("Folders › Save to master", "โฟลเดอร์ › บันทึกลงไฟล์หลัก")),
                        pair("keyman save --new", t("Sidebar › New project from folder", "แถบข้าง › โปรเจกต์ใหม่จากโฟลเดอร์")),
                        pair("keyman export", t("Copy export", "คัดลอก export")),
                    ],
                ));
            (t("CLI commands", "คำสั่ง CLI"), body)
        }
    };
    nav_dialog(p, "help", t("Help", "ความช่วยเหลือ"), nav, title, body)
}

fn confirm_dialog(s: &App, p: Paint) -> El {
    let path = s
        .folder()
        .map(|f| display_path(&f.join(".env")))
        .unwrap_or_default();
    col()
        .key("confirm")
        .role("alertdialog")
        .label(t("Overwrite .env?", "เขียนทับ .env หรือไม่"))
        .w(440.0)
        .padding(22.0)
        .gap(14.0)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.border)
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
        .child(icon_tile(&ic::ALERT, 40.0, p.yellow, p.dark))
        .child(text(t("Overwrite .env?", "เขียนทับ .env หรือไม่")).font(H2).color(p.text))
        .child(
            text(tf!(
                "{} differs from the master file. Writing replaces it; changes made only in .env are lost. Resolve the differences to keep them.",
                "{} ต่างจากไฟล์หลัก การเขียนจะแทนที่ไฟล์นี้ และค่าที่มีเฉพาะใน .env จะหายไป ใช้ \"แก้ไขความต่าง\" เพื่อเก็บไว้",
                path
            ))
                .font(BODY)
                .color(p.muted)
                .wrap(),
        )
        .child(
            row()
                .w_full()
                .gap(8.0)
                .pt(6.0)
                .child(button(p, "confirm-resolve", t("Resolve instead…", "แก้ไขความต่างแทน…"), Some(&ic::MERGE), Kind::Ghost, true, |s, _| {
                    s.dialog = None;
                    if let Some(folder) = s.folder() {
                        s.open_resolver(&folder);
                    }
                }))
                .child(el().grow(1.0))
                .child(button(p, "confirm-cancel", t("Cancel", "ยกเลิก"), None, Kind::Secondary, true, |s, _| s.dialog = None))
                .child(button(p, "confirm-overwrite", t("Overwrite", "เขียนทับ"), Some(&ic::DOWNLOAD), Kind::Danger, true, |s, _| s.inject_now())),
        )
}

fn confirm_all_dialog(s: &App, p: Paint) -> El {
    let changing: Vec<String> = match s.variables() {
        Some(Ok(vars)) => s
            .tied()
            .iter()
            .filter(|f| core::env_would_change(vars, f))
            .map(|f| display_path(f))
            .collect(),
        _ => Vec::new(),
    };
    let mut list = col().key("confirm-all-list").w_full().gap(4.0);
    for path in &changing {
        list = list.child(
            row().gap(8.0).center_y().child(dot(p.yellow)).child(
                text(path.clone())
                    .font(MONO_SMALL)
                    .color(p.text)
                    .truncate()
                    .min_w(0.0),
            ),
        );
    }
    col()
        .key("confirm-all")
        .role("alertdialog")
        .label(t("Overwrite .env in tied folders?", "เขียนทับ .env ในโฟลเดอร์ที่ผูกไว้หรือไม่"))
        .w(460.0)
        .padding(22.0)
        .gap(14.0)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.border)
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
        .child(icon_tile(&ic::ALERT, 40.0, p.yellow, p.dark))
        .child(text(t("Overwrite .env in tied folders?", "เขียนทับ .env ในโฟลเดอร์ที่ผูกไว้หรือไม่")).font(H2).color(p.text))
        .child(
            text(if i18n::thai() {
                format!(
                    "{} โฟลเดอร์มี .env ที่ต่างจากไฟล์หลัก การเขียนจะแทนที่ไฟล์เหล่านี้ และค่าที่มีเฉพาะในนั้นจะหายไป",
                    changing.len()
                )
            } else {
                format!(
                    "{} {} a .env that differs from the master file. Writing replaces them; changes made only there are lost.",
                    count(changing.len(), "folder", "folders", ""),
                    if changing.len() == 1 { "has" } else { "have" }
                )
            })
            .font(BODY)
            .color(p.muted)
            .wrap(),
        )
        .child(list)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .pt(6.0)
                .child(el().grow(1.0))
                .child(button(p, "confirm-all-cancel", t("Cancel", "ยกเลิก"), None, Kind::Secondary, true, |s, _| s.dialog = None))
                .child(button(p, "confirm-all-write", t("Write to all", "เขียนทั้งหมด"), Some(&ic::DOWNLOAD), Kind::Danger, true, |s, _| s.write_all_now())),
        )
}
