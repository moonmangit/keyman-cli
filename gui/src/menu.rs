//! The action panel a tied folder opens on right-click (or its ⋯ button):
//! every folder action in one place, run against the folder that was clicked.

use std::path::PathBuf;

use keyman_cli::ui::display_path;
use rune::{Color, El, Icon, col, el, icon, row, text};

use crate::App;
use crate::core::{self, FolderStatus};
use crate::diagram::status_look;
use crate::i18n::t;
use crate::icons as ic;
use crate::widgets::*;

pub const MENU_W: f32 = 268.0;
const ITEM_H: f32 = 32.0;

/// One row of the panel. `run` gets the clicked folder.
fn item(
    p: Paint,
    key: &str,
    glyph: &'static Icon,
    label: &str,
    tint: Option<Color>,
    enabled: bool,
    run: impl Fn(&mut App) + 'static,
) -> El {
    let ink = tint.unwrap_or(p.text);
    let mut r = row()
        .key(key)
        .role("menuitem")
        .label(label)
        .w_full()
        .h(ITEM_H)
        .px(10.0)
        .gap(10.0)
        .center_y()
        .radius(6.0)
        .disabled(!enabled)
        .child(
            icon(glyph, 15.0)
                .color(if tint.is_some() { ink } else { p.muted })
                .a11y_hidden(),
        )
        .child(
            text(label.to_owned())
                .font(BODY)
                .color(ink)
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .a11y_hidden(),
        );
    r = if enabled {
        let hover = tint.map_or(p.accent.alpha(if p.dark { 0.22 } else { 0.12 }), |c| {
            c.alpha(0.14)
        });
        r.focusable(true).hover(hover).press(p.press).on_press(run)
    } else {
        r.opacity(0.4)
    };
    r
}

fn separator(p: Paint, key: &str) -> El {
    el().key(key).w_full().py(4.0).child(hairline(p, "line"))
}

/// The panel with its top-left at `at`, kept inside the window (`window` is its size).
pub fn folder_menu(s: &App, p: Paint, at: (f32, f32), window: (f32, f32)) -> El {
    let Some(menu) = &s.menu else { return el() };
    let folder = menu.folder.clone();
    let vars = match s.variables() {
        Some(Ok(vars)) => vars.clone(),
        _ => Vec::new(),
    };
    let status = core::folder_status(&vars, &folder);
    let (tint, label) = status_look(p, status);
    let exists = !matches!(status, FolderStatus::Missing);
    let has_env = matches!(
        status,
        FolderStatus::InSync | FolderStatus::Differs(_) | FolderStatus::Unreadable
    );
    let differs = matches!(status, FolderStatus::Differs(_));
    let is_active = s.folder().as_deref() == Some(folder.as_path());
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| display_path(&folder));

    let on = |action: fn(&mut App)| {
        let folder: PathBuf = folder.clone();
        move |s: &mut App| s.on_folder(&folder, action)
    };
    let (copy_path, reveal_path, untie_path, active_path) = (
        folder.clone(),
        folder.clone(),
        folder.clone(),
        folder.clone(),
    );

    let panel = col()
        .key("folder-menu")
        .role("menu")
        .label(t("Folder actions", "การทำงานของโฟลเดอร์"))
        .w(MENU_W)
        .padding(6.0)
        .gap(1.0)
        .radius(10.0)
        .fill(p.raised)
        .border(1.0, p.border)
        .shadow(
            10.0,
            30.0,
            Color::BLACK.alpha(if p.dark { 0.5 } else { 0.18 }),
        )
        .child(
            row()
                .key("menu-head")
                .w_full()
                .px(8.0)
                .pt(6.0)
                .pb(8.0)
                .gap(10.0)
                .center_y()
                .child(icon_tile(&ic::FOLDER, 30.0, tint, p.dark))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(1.0)
                        .child(text(name).font(BODY_STRONG).color(p.text).truncate())
                        .child(
                            text(display_path(&folder))
                                .font(SMALL)
                                .color(p.faint)
                                .truncate(),
                        ),
                )
                .child(badge(p, &label, tint)),
        )
        .child(separator(p, "sep-head"))
        .child(item(
            p,
            "mi-active",
            &ic::CHECK_CIRCLE,
            if is_active {
                t("Active folder", "โฟลเดอร์ที่ใช้งานอยู่")
            } else {
                t("Make active", "ตั้งเป็นโฟลเดอร์ที่ใช้งาน")
            },
            None,
            !is_active,
            move |s| {
                s.menu = None;
                s.set_active(&active_path);
            },
        ))
        .child(item(
            p,
            "mi-write",
            &ic::DOWNLOAD,
            t("Write .env", "เขียน .env"),
            None,
            exists,
            on(App::inject),
        ))
        .child(item(
            p,
            "mi-compare",
            &ic::COMPARE,
            t("Compare with master", "เปรียบเทียบกับไฟล์หลัก"),
            None,
            has_env,
            on(App::diff),
        ))
        .child(item(
            p,
            "mi-resolve",
            &ic::MERGE,
            t("Resolve differences…", "แก้ไขความต่าง…"),
            differs.then_some(p.yellow),
            differs,
            on(App::resolve_active),
        ))
        .child(item(
            p,
            "mi-save",
            &ic::UPLOAD,
            t("Save .env to master…", "บันทึก .env ลงไฟล์หลัก…"),
            None,
            has_env,
            on(App::preview_save),
        ))
        .child(separator(p, "sep-mid"))
        .child(item(
            p,
            "mi-reveal",
            &ic::EXTERNAL,
            if cfg!(target_os = "macos") {
                t("Show in Finder", "แสดงใน Finder")
            } else {
                t("Show in Explorer", "แสดงใน Explorer")
            },
            None,
            exists,
            move |s| {
                s.menu = None;
                if let Err(error) = core::reveal_in_file_manager(&reveal_path) {
                    s.fail(error);
                }
            },
        ))
        .child(
            row()
                .key("mi-copy")
                .role("menuitem")
                .label(t("Copy path", "คัดลอกพาธ"))
                .w_full()
                .h(ITEM_H)
                .px(10.0)
                .gap(10.0)
                .center_y()
                .radius(6.0)
                .focusable(true)
                .hover(p.accent.alpha(if p.dark { 0.22 } else { 0.12 }))
                .press(p.press)
                .on_press_tx(move |s: &mut App, tx| {
                    s.menu = None;
                    tx.copy(copy_path.to_string_lossy().into_owned());
                    s.ok(t("Copied the folder path", "คัดลอกพาธโฟลเดอร์แล้ว"));
                })
                .child(icon(&ic::COPY, 15.0).color(p.muted).a11y_hidden())
                .child(
                    text(t("Copy path", "คัดลอกพาธ"))
                        .font(BODY)
                        .color(p.text)
                        .a11y_hidden(),
                ),
        )
        .child(separator(p, "sep-end"))
        .child(item(
            p,
            "mi-untie",
            &ic::CLOSE,
            t("Untie folder", "เลิกผูกโฟลเดอร์"),
            Some(p.red),
            true,
            move |s| {
                s.menu = None;
                s.untie(&untie_path);
            },
        ));

    // Roughly the panel's height, to keep it on screen near the bottom edge.
    let height = 60.0 + 8.0 * ITEM_H + 3.0 * 9.0 + 12.0;
    let (x, y) = at;
    let x = x.min(window.0 - MENU_W - 12.0).max(8.0);
    let y = if y + height > window.1 - 12.0 {
        (y - height).max(8.0)
    } else {
        y
    };

    // A clear layer over the window: a click outside closes the panel.
    el().key("menu-layer")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            panel
                .absolute(x, y)
                .modal(true, |s: &mut App| s.menu = None),
        )
}

pub const PROJECT_MENU_W: f32 = 230.0;

/// The project page's ⋯ menu: **Unlink** (this computer only) and **Delete from cloud**.
pub fn project_menu(s: &App, p: Paint, project: &str, at: (f32, f32), window: (f32, f32)) -> El {
    let linked = s.link_of(project).is_some();
    let signed_in = matches!(s.cloud, crate::cloud::CloudState::SignedIn { .. });
    let (unlink_name, delete_name) = (project.to_owned(), project.to_owned());
    let panel = col()
        .key("project-menu")
        .role("menu")
        .label(t("Project actions", "การทำงานของโปรเจกต์"))
        .w(PROJECT_MENU_W)
        .padding(6.0)
        .gap(1.0)
        .radius(10.0)
        .fill(p.raised)
        .border(1.0, p.border)
        .shadow(
            10.0,
            30.0,
            Color::BLACK.alpha(if p.dark { 0.5 } else { 0.18 }),
        )
        .child(item(
            p,
            "pm-unlink",
            &ic::CLOUD,
            t("Unlink", "เลิกเชื่อม"),
            None,
            linked && crate::cloud_views::can_unlink(&s.busy),
            move |s| s.cloud_unlink(&unlink_name),
        ))
        .child(separator(p, "pm-sep"))
        .child(item(
            p,
            "pm-delete",
            &ic::CLOSE,
            t("Delete from cloud", "ลบออกจากคลาวด์"),
            Some(p.red),
            linked && signed_in && crate::cloud::can_start(&s.busy),
            move |s| s.ask_cloud_delete(&delete_name),
        ));
    let height = 12.0 + 2.0 * ITEM_H + 9.0;
    let (x, y) = at;
    let x = x.min(window.0 - PROJECT_MENU_W - 12.0).max(8.0);
    let y = if y + height > window.1 - 12.0 {
        (y - height).max(8.0)
    } else {
        y
    };
    el().key("project-menu-layer")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(
            panel
                .absolute(x, y)
                .modal(true, |s: &mut App| s.project_menu = None),
        )
}
