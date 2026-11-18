//! The tied-folders diagram, drawn top-down so it fits a half-width column:
//! the master file feeds the project, and a trunk from the project branches to
//! each tied folder, every branch coloured by that folder's .env status.

use std::path::PathBuf;

use keyman_cli::ui::display_path;
use rune::{Color, El, LineCap, Path as Curve, Shape, canvas, col, row, text};

use crate::core::FolderStatus;
use crate::i18n::{count, t};
use crate::icons as ic;
use crate::widgets::*;
use crate::{App, FolderMenu, MenuAt};

const NODE_H: f32 = 60.0;
const GAP: f32 = 10.0;
/// Space between the project node and the first folder.
const DROP: f32 = 16.0;
/// Height of the master → project link.
const LINK_H: f32 = 26.0;
/// The trunk runs under the nodes' icons: 12 pt padding plus half a 34 pt tile.
const TRUNK_X: f32 = 29.0;
/// Width of the branch area left of the folder nodes.
const FAN_W: f32 = 58.0;
const ELBOW: f32 = 12.0;

pub fn status_look(p: Paint, status: FolderStatus) -> (Color, String) {
    match status {
        FolderStatus::InSync => (p.green, t("In sync", "ตรงกัน").into()),
        FolderStatus::Differs(n) => (p.yellow, count(n, "difference", "differences", "จุดที่ต่าง")),
        FolderStatus::NoEnv => (p.accent, t("No .env yet", "ยังไม่มี .env").into()),
        FolderStatus::Missing => (p.red, t("Missing", "หาไม่พบ").into()),
        FolderStatus::Unreadable => (p.red, t("Unreadable", "อ่านไม่ได้").into()),
        FolderStatus::NoStage => (p.muted, t("Pick a stage", "เลือกสเตจ").into()),
    }
}

fn node(
    p: Paint,
    key: &str,
    glyph: &'static rune::Icon,
    tint: Color,
    title: String,
    subtitle: String,
) -> El {
    row()
        .key(key)
        .w_full()
        .h(NODE_H)
        .px(12.0)
        .gap(10.0)
        .center_y()
        .radius(12.0)
        .fill(p.card)
        .border(1.0, p.border)
        .shadow(1.0, 2.0, p.shadow.alpha(0.4))
        .child(icon_tile(glyph, 34.0, tint, p.dark))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(text(title).font(BODY_STRONG).color(p.text).truncate())
                .child(text(subtitle).font(SMALL).color(p.muted).truncate()),
        )
}

/// A small filled circle, for connector ends.
fn disc(shape: Shape, c: [f32; 2], r: f32, color: Color) -> Shape {
    let points: Vec<[f32; 2]> = (0..16)
        .map(|i| {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect();
    shape.fill(points, color)
}

fn elbow(y: f32) -> Curve {
    Curve::new()
        .move_to([TRUNK_X, y - ELBOW])
        .quad_to([TRUNK_X, y], [TRUNK_X + ELBOW, y])
        .line_to([FAN_W - 4.0, y])
}

pub fn tie_diagram(s: &App, p: Paint, project: &str) -> El {
    let folders = s.tied();
    let active = s.folder();
    let statuses: Vec<FolderStatus> = folders.iter().map(|f| s.folder_status(f)).collect();
    let n = folders.len().max(1);
    let fan_h = DROP + n as f32 * NODE_H + (n - 1) as f32 * GAP;
    let row_y = |i: usize| DROP + i as f32 * (NODE_H + GAP) + NODE_H / 2.0;
    let trunk = p.accent.alpha(0.55);

    // Master file → project: a short vertical link under the icons.
    let link = disc(
        disc(
            Shape::new().line(
                [TRUNK_X, 4.0],
                [TRUNK_X, LINK_H - 4.0],
                2.0,
                LineCap::Round,
                trunk,
            ),
            [TRUNK_X, 3.0],
            3.0,
            trunk,
        ),
        [TRUNK_X, LINK_H - 3.0],
        3.0,
        trunk,
    );

    // Project → folders: a neutral trunk, then a coloured elbow into each folder.
    let last_y = row_y(n - 1);
    let mut fan = disc(Shape::new(), [TRUNK_X, 3.0], 3.0, trunk).line(
        [TRUNK_X, 4.0],
        [TRUNK_X, last_y - ELBOW],
        2.0,
        LineCap::Round,
        trunk,
    );
    if folders.is_empty() {
        fan = fan.stroke_path(&elbow(last_y), 1.5, LineCap::Round, p.faint.alpha(0.6));
    }
    for (i, status) in statuses.iter().enumerate() {
        let (color, _) = status_look(p, *status);
        let color = if *status == FolderStatus::Missing {
            color.alpha(0.5)
        } else {
            color
        };
        let width = if active.as_deref() == Some(folders[i].as_path()) {
            2.6
        } else {
            2.0
        };
        let y = row_y(i);
        fan = disc(
            fan.stroke_path(&elbow(y), width, LineCap::Round, color),
            [FAN_W - 3.0, y],
            3.5,
            color,
        );
    }

    let master_name = s
        .master
        .as_ref()
        .ok()
        .and_then(|m| m.path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let master_node = node(
        p,
        "dg-master",
        &ic::FILE,
        p.muted,
        master_name,
        t("Master file", "ไฟล์หลัก").into(),
    );
    let project_node = node(
        p,
        "dg-project",
        &ic::KEY,
        p.accent,
        project.to_owned(),
        match s.stages().len() {
            0 => count(
                s.variables().and_then(Result::ok).map_or(0, |v| v.len()),
                "variable",
                "variables",
                "ตัวแปร",
            ),
            n => count(n, "stage", "stages", "สเตจ"),
        },
    )
    .border(1.5, p.accent.alpha(0.6));

    let mut column = col()
        .key("dg-folders")
        .grow(1.0)
        .min_w(0.0)
        .pt(DROP)
        .gap(GAP);
    if folders.is_empty() {
        column = column.child(
            row()
                .key("dg-empty")
                .role("button")
                .label(t("Tie a folder", "ผูกโฟลเดอร์"))
                .w_full()
                .h(NODE_H)
                .px(12.0)
                .gap(10.0)
                .center_y()
                .radius(12.0)
                .border(1.5, p.faint.alpha(0.6))
                .border_dash(6.0, 4.0)
                .focusable(true)
                .hover(p.hover)
                .on_press_tx(crate::tie_folder_dialog)
                .child(icon_tile(&ic::PLUS, 34.0, p.muted, p.dark))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(
                            text(t("Tie a folder", "ผูกโฟลเดอร์"))
                                .font(BODY_STRONG)
                                .color(p.text),
                        )
                        .child(
                            text(t(
                                "Where this project's code lives",
                                "โฟลเดอร์ที่เก็บโค้ดของโปรเจกต์นี้",
                            ))
                            .font(SMALL)
                            .color(p.muted)
                            .truncate(),
                        ),
                ),
        );
    }
    for (i, folder) in folders.iter().enumerate() {
        let (color, label) = status_look(p, statuses[i]);
        let is_active = active.as_deref() == Some(folder.as_path());
        let name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| display_path(folder));
        let target: PathBuf = folder.clone();
        let (right_click, dots) = (folder.clone(), folder.clone());
        let dots_key = format!("dg-more-{i}");
        let below = dots_key.clone();
        let mut folder_node = node(
            p,
            &format!("dg-folder-{i}"),
            &ic::FOLDER,
            color,
            name,
            // The stage this folder's .env is written from, before its path.
            match s.folder_stage(folder) {
                Some(stage) => format!("{stage} · {}", display_path(folder)),
                None => display_path(folder),
            },
        )
        .role("button")
        .label(tf!("Folder {}", "โฟลเดอร์ {}", display_path(folder)))
        .checked(is_active)
        .focusable(true)
        .hover(p.hover)
        .press(p.press)
        .on_press(move |s: &mut App| s.set_active(&target))
        .on_secondary_press(move |s: &mut App, at, _| {
            s.menu = Some(FolderMenu {
                folder: right_click.clone(),
                at: MenuAt::Point(at.x, at.y),
            });
        })
        .child(badge(p, &label, color))
        .child(
            icon_button(
                p,
                &dots_key,
                &ic::MORE,
                t("Folder actions", "การทำงานของโฟลเดอร์"),
                move |s, _| {
                    s.menu = Some(FolderMenu {
                        folder: dots.clone(),
                        at: MenuAt::Below(below.clone()),
                    });
                },
            )
            .size(26.0, 26.0),
        );
        if is_active {
            folder_node = folder_node.border(2.0, p.accent);
        }
        column = column.child(folder_node);
    }

    let graph = col()
        .key("dg-graph")
        .w_full()
        .child(master_node)
        .child(canvas(link).key("dg-link").w_full().h(LINK_H))
        .child(project_node)
        .child(
            row()
                .key("dg-fan-row")
                .w_full()
                .child(canvas(fan).key("dg-fan").size(FAN_W, fan_h))
                .child(column),
        );

    let writable = folders.iter().any(|f| f.is_dir());
    let in_sync = statuses
        .iter()
        .filter(|st| **st == FolderStatus::InSync)
        .count();
    let summary = if folders.is_empty() {
        t("No folders tied yet", "ยังไม่ได้ผูกโฟลเดอร์").to_owned()
    } else {
        tf!(
            "{} · {} in sync · right-click for actions",
            "{} · ตรงกัน {} · คลิกขวาเพื่อดูคำสั่ง",
            count(folders.len(), "folder", "folders", "โฟลเดอร์"),
            in_sync
        )
    };
    let legend_item = |key: &str, color: Color, label: &str| {
        row()
            .key(key)
            .gap(6.0)
            .center_y()
            .child(dot(color))
            .child(text(label.to_owned()).font(SMALL).color(p.muted))
    };

    card(p, "ties")
        .child(
            row()
                .key("ties-head")
                .w_full()
                .px(18.0)
                .py(14.0)
                .gap(8.0)
                .center_y()
                .border_sides([0.0, 0.0, 1.0, 0.0], p.separator)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(text(t("Folders", "โฟลเดอร์")).font(H2).color(p.text))
                        .child(text(summary).font(SMALL).color(p.muted).truncate()),
                )
                .child(button(
                    p,
                    "tie-folder",
                    t("Tie…", "ผูก…"),
                    Some(&ic::PLUS),
                    Kind::Secondary,
                    true,
                    crate::tie_folder_dialog,
                ))
                .child(button(
                    p,
                    "write-all",
                    t("Write to all", "เขียนทั้งหมด"),
                    Some(&ic::DOWNLOAD),
                    Kind::Primary,
                    writable,
                    |s, _| s.write_all(),
                )),
        )
        .child(col().key("dg-body").w_full().padding(18.0).child(graph))
        .child(
            row()
                .key("legend")
                .w_full()
                .px(18.0)
                .py(12.0)
                .gap(14.0)
                .center_y()
                .border_sides([1.0, 0.0, 0.0, 0.0], p.separator)
                .child(legend_item("lg-sync", p.green, t("In sync", "ตรงกัน")))
                .child(legend_item("lg-diff", p.yellow, t("Differs", "ต่างกัน")))
                .child(legend_item("lg-none", p.accent, t("No .env", "ไม่มี .env")))
                .child(legend_item("lg-missing", p.red, t("Missing", "หาไม่พบ"))),
        )
}
