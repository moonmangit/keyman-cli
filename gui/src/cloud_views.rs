//! Keyman Cloud screens: the Account page, the first-use card, the title-bar cloud icon, and
//! the dialogs for the questions a cloud task asks (vault password, recovery code).

use std::sync::atomic::Ordering;

use rune::{Color, El, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};
use zeroize::Zeroizing;

use crate::cloud::{self, Ask, Busy, CloudState};
use crate::i18n::{count, t};
use crate::icons as ic;
use crate::widgets::*;
use crate::{App, Dialog, SettingsPage};

/// The shortest vault password Keyman accepts, in characters.
const MIN_PASSWORD: usize = 10;

/// The recovery code, shown large.
const CODE: TextStyle = TextStyle::new(19.0, 500.0).mono();

/// Why the two new vault password entries cannot be used, or `None` when they can.
pub fn new_password_error(a: &str, b: &str) -> Option<&'static str> {
    if a.chars().count() < MIN_PASSWORD {
        Some(t(
            "The password must have 10 characters or more.",
            "รหัสผ่านต้องมีอย่างน้อย 10 ตัวอักษร",
        ))
    } else if a != b {
        Some(t(
            "The two passwords are not the same.",
            "รหัสผ่านทั้งสองช่องไม่ตรงกัน",
        ))
    } else {
        None
    }
}

/// The first-use card shows when a master file is loaded, nobody is signed in, the person
/// did not skip it before, and no dialog is open.
pub fn show_first_use(loaded: bool, cloud: &CloudState, done: bool, dialog_open: bool) -> bool {
    loaded && *cloud == CloudState::SignedOut && !done && !dialog_open
}

/// Whether the dialog fields hold a complete answer to `ask`.
pub fn can_answer(ask: &Ask, first: &str, second: &str) -> bool {
    match ask {
        Ask::NewPassword => new_password_error(first, second).is_none(),
        Ask::ShowRecoveryCode(code) => !first.trim().is_empty() && same_code(first, code),
        Ask::Password { .. } | Ask::RecoveryCode => !first.is_empty(),
        // The cloud resolver answers these.
        Ask::Resolve { .. } => false,
    }
}

/// The new-password error to show now: nothing before the person types.
fn shown_password_error(a: &str, b: &str) -> Option<&'static str> {
    let error = new_password_error(a, b)?;
    let short = a.chars().count() < MIN_PASSWORD;
    ((short && !a.is_empty()) || (!short && !b.is_empty())).then_some(error)
}

/// A recovery code's characters as the library compares them (`normalize_recovery_code`:
/// no spaces or dashes, upper case), without a copy: a dialog checks the code on each frame.
fn code_chars(code: &str) -> impl Iterator<Item = char> + '_ {
    code.chars()
        .filter(|c| *c != ' ' && *c != '-')
        .flat_map(char::to_uppercase)
}

fn same_code(typed: &str, code: &str) -> bool {
    code_chars(typed).eq(code_chars(code))
}

/// The recovery code in two lines of four groups, wiped when dropped.
fn code_lines(code: &str) -> Vec<Zeroizing<String>> {
    let groups: Vec<&str> = code.split('-').collect();
    groups
        .chunks(4)
        .map(|line| Zeroizing::new(line.join("-")))
        .collect()
}

// ── Title bar ───────────────────────────────────────────────────────────────

/// The title-bar cloud icon: signed out, signed in, or busy. A click opens Settings › Account.
pub fn cloud_button(s: &App, p: Paint) -> El {
    let (glyph, tint, tip) = match (&s.busy, &s.cloud) {
        (Some(busy), _) => (
            &ic::CLOUD_UP,
            p.accent,
            format!("Keyman Cloud · {}", busy.label),
        ),
        (None, CloudState::SignedIn { email }) => (
            &ic::CLOUD_CHECK,
            p.green,
            tf!(
                "Keyman Cloud · signed in as {}",
                "Keyman Cloud · ลงชื่อเข้าใช้เป็น {}",
                email
            ),
        ),
        (None, CloudState::SignedOut) => (
            &ic::CLOUD,
            p.muted,
            t(
                "Keyman Cloud · not signed in",
                "Keyman Cloud · ยังไม่ได้ลงชื่อเข้าใช้",
            )
            .to_owned(),
        ),
    };
    el().key("tb-cloud")
        .role("button")
        .label(&tip)
        .help(&tip)
        .size(30.0, 30.0)
        .center()
        .radius(7.0)
        .focusable(true)
        .hover(p.hover)
        .press(p.press)
        .on_press(|s: &mut App| s.dialog = Some(Dialog::Settings(SettingsPage::Account)))
        .child(icon(glyph, 17.0).color(tint).a11y_hidden())
}

// ── First-use card ──────────────────────────────────────────────────────────

pub fn first_use_card(s: &App, p: Paint) -> El {
    let idle = cloud::can_start(&s.busy);
    // The Account page shows the sign-in while it runs: the browser link and Cancel.
    let sign_in = |provider: &'static str| {
        move |s: &mut App, _: &mut Tx| {
            s.cloud_sign_in(provider);
            s.dialog = Some(Dialog::Settings(SettingsPage::Account));
        }
    };
    card(p, "cloud-first-use")
        .padding(16.0)
        .gap(14.0)
        .child(
            row()
                .w_full()
                .gap(14.0)
                .center_y()
                .child(icon_tile(&ic::CLOUD, 40.0, p.accent, p.dark))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(3.0)
                        .child(
                            text(t(
                                "Keep an encrypted copy in Keyman Cloud?",
                                "เก็บสำเนาที่เข้ารหัสไว้ใน Keyman Cloud ไหม",
                            ))
                            .font(H2)
                            .color(p.text)
                            .wrap(),
                        )
                        .child(
                            text(t(
                                "Only you can read it. You can also sign in later in Settings › Account.",
                                "มีเพียงคุณที่อ่านได้ และลงชื่อเข้าใช้ภายหลังได้ที่ การตั้งค่า › บัญชี",
                            ))
                            .font(SMALL)
                            .color(p.muted)
                            .wrap(),
                        ),
                ),
        )
        .child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(button(
                    p,
                    "first-github",
                    t("Sign in with GitHub", "ลงชื่อเข้าใช้ด้วย GitHub"),
                    None,
                    Kind::Primary,
                    idle,
                    sign_in("github"),
                ))
                .child(button(
                    p,
                    "first-google",
                    t("Sign in with Google", "ลงชื่อเข้าใช้ด้วย Google"),
                    None,
                    Kind::Secondary,
                    idle,
                    sign_in("google"),
                ))
                .child(el().grow(1.0))
                .child(button(
                    p,
                    "first-skip",
                    t("Skip", "ข้าม"),
                    None,
                    Kind::Ghost,
                    true,
                    |s, _| s.skip_cloud_prompt(),
                )),
        )
}

// ── Settings › Account ──────────────────────────────────────────────────────

/// A text button for a less common action.
fn link_button(
    p: Paint,
    key: &str,
    label: &str,
    enabled: bool,
    f: impl Fn(&mut App, &mut Tx) + 'static,
) -> El {
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .h(26.0)
        .px(6.0)
        .center_y()
        .radius(6.0)
        .disabled(!enabled)
        .child(
            text(label.to_owned())
                .font(BODY_STRONG)
                .color(p.accent)
                .a11y_hidden(),
        );
    if enabled {
        b.focusable(true)
            .hover(p.hover)
            .press(p.press)
            .on_press_tx(f)
    } else {
        b.opacity(0.45)
    }
}

/// The running task: its label, **Cancel**, and the sign-in link while the browser waits.
fn busy_box(p: Paint, busy: &Busy) -> El {
    let cancelling = busy.cancel.load(Ordering::SeqCst);
    let mut b = col()
        .key("cloud-busy")
        .w_full()
        .padding(16.0)
        .gap(12.0)
        .radius(10.0)
        .fill(p.card)
        .border(1.0, p.border)
        .child(
            row()
                .w_full()
                .gap(10.0)
                .center_y()
                .child(icon(&ic::CLOUD_UP, 18.0).color(p.accent).a11y_hidden())
                .child(
                    text(busy.label)
                        .font(BODY_STRONG)
                        .color(p.text)
                        .role("status")
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(button(
                    p,
                    "cloud-cancel",
                    t("Cancel", "ยกเลิก"),
                    None,
                    Kind::Secondary,
                    !cancelling,
                    |s, _| s.cloud_cancel(),
                )),
        );
    if let Some(link) = &busy.link {
        b = b.child(
            col()
                .key("cloud-link")
                .w_full()
                .gap(6.0)
                .child(
                    text(t(
                        "If the browser does not open, open this link:",
                        "ถ้าเบราว์เซอร์ไม่เปิดขึ้นมา ให้เปิดลิงก์นี้:",
                    ))
                    .font(SMALL)
                    .color(p.muted)
                    .wrap(),
                )
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .center_y()
                        .child(
                            row()
                                .key("cloud-link-box")
                                .h(30.0)
                                .px(10.0)
                                .center_y()
                                .grow(1.0)
                                .min_w(0.0)
                                .radius(7.0)
                                .fill(field_fill(p))
                                .border(1.0, p.border)
                                .child(
                                    text(link.clone())
                                        .font(MONO_SMALL)
                                        .color(p.text)
                                        .truncate()
                                        .selectable(true)
                                        .grow(1.0)
                                        .min_w(0.0),
                                ),
                        )
                        .child(button(
                            p,
                            "cloud-copy-link",
                            t("Copy", "คัดลอก"),
                            Some(&ic::COPY),
                            Kind::Secondary,
                            true,
                            |s, tx| {
                                if let Some(link) = s.busy.as_ref().and_then(|b| b.link.clone()) {
                                    tx.copy(link);
                                    s.ok(t("Copied the link.", "คัดลอกลิงก์แล้ว"));
                                }
                            },
                        )),
                ),
        );
    }
    b
}

/// Settings › Account: sign in, or the signed-in account, and the running task.
pub fn account_page(s: &App, p: Paint) -> El {
    let idle = cloud::can_start(&s.busy);
    let mut page = col().w_full().gap(22.0).child(
        row()
            .key("cloud-head")
            .w_full()
            .gap(16.0)
            .center_y()
            .child(icon_tile(&ic::CLOUD, 48.0, p.accent, p.dark))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(4.0)
                    .child(text("Keyman Cloud").font(H1).color(p.text))
                    .child(
                        text(t(
                            "Keep an encrypted copy of your projects. The server cannot read them.",
                            "เก็บสำเนาที่เข้ารหัสของโปรเจกต์ไว้บนคลาวด์ เซิร์ฟเวอร์อ่านข้อมูลนี้ไม่ได้",
                        ))
                        .font(BODY)
                        .color(p.muted)
                        .wrap(),
                    ),
            ),
    );
    if let Some(busy) = &s.busy {
        page = page.child(busy_box(p, busy));
    }
    page = page.child(match &s.cloud {
        CloudState::SignedIn { email } => signed_in_group(p, email, idle),
        CloudState::SignedOut if s.recover_pick => recover_group(p, idle),
        CloudState::SignedOut => sign_in_group(p, idle),
    });
    if !s.cloud_store {
        page = page.child(
            row()
                .key("cloud-no-store")
                .w_full()
                .gap(8.0)
                .child(icon(&ic::INFO, 14.0).color(p.faint))
                .child(
                    text(t(
                        "This computer has no credential store. Keyman asks for the vault password at each cloud action.",
                        "เครื่องนี้ไม่มีที่เก็บรหัสผ่านของระบบ Keyman จึงถามรหัสผ่านตู้นิรภัยทุกครั้งที่ทำงานกับคลาวด์",
                    ))
                    .font(SMALL)
                    .color(p.muted)
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
                ),
        );
    }
    page
}

/// The two provider buttons, each running `action` with "github" or "google".
fn provider_buttons(p: Paint, key: &str, idle: bool, action: fn(&mut App, &'static str)) -> El {
    row()
        .key(key)
        .gap(8.0)
        .child(button(
            p,
            &format!("{key}-github"),
            t("Sign in with GitHub", "ลงชื่อเข้าใช้ด้วย GitHub"),
            None,
            Kind::Primary,
            idle,
            move |s, _| action(s, "github"),
        ))
        .child(button(
            p,
            &format!("{key}-google"),
            t("Sign in with Google", "ลงชื่อเข้าใช้ด้วย Google"),
            None,
            Kind::Secondary,
            idle,
            move |s, _| action(s, "google"),
        ))
}

fn sign_in_group(p: Paint, idle: bool) -> El {
    settings_group(
        p,
        "g-sign-in",
        t("SIGN IN", "ลงชื่อเข้าใช้"),
        vec![
            col()
                .key("sign-in-body")
                .w_full()
                .py(14.0)
                .gap(12.0)
                .child(
                    text(t(
                        "Use your GitHub or Google account. The first time, you make a vault password and get a recovery code.",
                        "ใช้บัญชี GitHub หรือ Google ครั้งแรกคุณจะตั้งรหัสผ่านตู้นิรภัยและได้รับรหัสกู้คืน",
                    ))
                    .font(SMALL)
                    .color(p.muted)
                    .wrap(),
                )
                .child(provider_buttons(p, "sign-in", idle, App::cloud_sign_in)),
            row()
                .key("recover-row")
                .w_full()
                .py(10.0)
                .gap(6.0)
                .center_y()
                .child(
                    text(t(
                        "Forgot the vault password?",
                        "ลืมรหัสผ่านตู้นิรภัยใช่ไหม",
                    ))
                    .font(SMALL)
                    .color(p.muted),
                )
                .child(link_button(
                    p,
                    "use-recovery",
                    t("Use recovery code", "ใช้รหัสกู้คืน"),
                    idle,
                    |s, _| s.recover_pick = true,
                )),
        ],
    )
}

/// **Use recovery code**: the provider to sign in with first.
fn recover_group(p: Paint, idle: bool) -> El {
    settings_group(
        p,
        "g-recover",
        t("RECOVERY CODE", "รหัสกู้คืน"),
        vec![
            col()
                .key("recover-body")
                .w_full()
                .py(14.0)
                .gap(12.0)
                .child(
                    text(t(
                        "Sign in, then type your recovery code and make a new vault password.",
                        "ลงชื่อเข้าใช้ แล้วพิมพ์รหัสกู้คืนและตั้งรหัสผ่านตู้นิรภัยใหม่",
                    ))
                    .font(SMALL)
                    .color(p.muted)
                    .wrap(),
                )
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .child(provider_buttons(p, "recover", idle, App::cloud_recover))
                        .child(el().grow(1.0))
                        .child(button(
                            p,
                            "recover-back",
                            t("Back", "ย้อนกลับ"),
                            None,
                            Kind::Ghost,
                            true,
                            |s, _| s.recover_pick = false,
                        )),
                ),
        ],
    )
}

fn signed_in_group(p: Paint, email: &str, idle: bool) -> El {
    settings_group(
        p,
        "g-account",
        t("ACCOUNT", "บัญชี"),
        vec![
            setting_row(
                p,
                "cloud-email",
                &tf!("Signed in as {}", "ลงชื่อเข้าใช้เป็น {}", email),
                t(
                    "Keyman keeps an encrypted copy of the projects that you link.",
                    "Keyman เก็บสำเนาที่เข้ารหัสของโปรเจกต์ที่คุณเชื่อมไว้",
                ),
                button(
                    p,
                    "cloud-sign-out",
                    t("Sign out", "ออกจากระบบ"),
                    None,
                    Kind::Secondary,
                    idle,
                    |s, _| s.dialog = Some(Dialog::ConfirmSignOut),
                ),
            ),
            setting_row(
                p,
                "cloud-recover",
                t("Forgot the vault password?", "ลืมรหัสผ่านตู้นิรภัยใช่ไหม"),
                t(
                    "Type your recovery code and make a new vault password.",
                    "พิมพ์รหัสกู้คืนแล้วตั้งรหัสผ่านตู้นิรภัยใหม่",
                ),
                button(
                    p,
                    "cloud-use-recovery",
                    t("Use recovery code", "ใช้รหัสกู้คืน"),
                    Some(&ic::KEY),
                    Kind::Secondary,
                    idle,
                    |s, _| s.cloud_recover_signed_in(),
                ),
            ),
            setting_row(
                p,
                "cloud-password",
                t("Vault password", "รหัสผ่านตู้นิรภัย"),
                t(
                    "Keyman uses it to encrypt your secrets.",
                    "Keyman ใช้รหัสผ่านนี้เข้ารหัสความลับของคุณ",
                ),
                button(
                    p,
                    "cloud-change-password",
                    t("Change vault password", "เปลี่ยนรหัสผ่านตู้นิรภัย"),
                    Some(&ic::LOCK),
                    Kind::Secondary,
                    idle,
                    |s, _| s.cloud_change_password(),
                ),
            ),
        ],
    )
}

/// **Sign out** asks first.
pub fn confirm_sign_out(p: Paint) -> El {
    let back = |s: &mut App, _: &mut Tx| s.dialog = Some(Dialog::Settings(SettingsPage::Account));
    col()
        .key("confirm-sign-out")
        .role("alertdialog")
        .label(t("Sign out of Keyman Cloud?", "ออกจากระบบ Keyman Cloud ไหม"))
        .w(440.0)
        .padding(22.0)
        .gap(14.0)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.border)
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
        .child(icon_tile(&ic::CLOUD, 40.0, p.yellow, p.dark))
        .child(
            text(t("Sign out of Keyman Cloud?", "ออกจากระบบ Keyman Cloud ไหม"))
                .font(H2)
                .color(p.text),
        )
        .child(
            text(t(
                "Keyman removes the sign-in and the vault key from this computer. Your projects and their cloud links stay. To sign in again, you need the vault password.",
                "Keyman จะลบการลงชื่อเข้าใช้และกุญแจตู้นิรภัยออกจากเครื่องนี้ โปรเจกต์และการเชื่อมกับคลาวด์ยังอยู่ หากต้องการลงชื่อเข้าใช้อีกครั้ง คุณต้องใช้รหัสผ่านตู้นิรภัย",
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
                .child(el().grow(1.0))
                .child(button(
                    p,
                    "sign-out-cancel",
                    t("Cancel", "ยกเลิก"),
                    None,
                    Kind::Secondary,
                    true,
                    back,
                ))
                .child(button(
                    p,
                    "sign-out-confirm",
                    t("Sign out", "ออกจากระบบ"),
                    None,
                    Kind::Danger,
                    true,
                    move |s, tx| {
                        back(s, tx);
                        s.cloud_sign_out();
                    },
                )),
        )
}

// ── Question dialogs ────────────────────────────────────────────────────────

fn field_fill(p: Paint) -> Color {
    if p.dark {
        Color::BLACK.alpha(0.2)
    } else {
        Color::WHITE
    }
}

/// A labelled secure field. `shown` draws the characters (a code to compare), but keeps the
/// other secure protections: no copy, no undo, no accessibility text, wiped buffers.
fn field(
    p: Paint,
    key: &str,
    label: &str,
    edit: &TextEdit,
    shown: bool,
    access: impl for<'a> Fn(&'a mut App) -> &'a mut TextEdit + 'static,
) -> El {
    let input = text_input(edit, if shown { MONO } else { BODY })
        .key(key)
        .label(label)
        .single_line()
        .color(p.text)
        .on_edit(access)
        .on_submit(|s: &mut App, _| s.cloud_answer())
        .secure()
        .revealed(shown)
        .grow(1.0)
        .min_w(0.0);
    col()
        .w_full()
        .gap(6.0)
        .child(text(label.to_owned()).font(SMALL).color(p.muted))
        .child(
            row()
                .key(format!("{key}-box"))
                .w_full()
                .h(34.0)
                .px(10.0)
                .center_y()
                .radius(8.0)
                .fill(field_fill(p))
                .border(1.0, p.border)
                .child(input),
        )
}

/// A short line under the fields: an error or a warning.
fn note(key: &str, glyph: &'static rune::Icon, tint: Color, message: String) -> El {
    row()
        .key(key)
        .w_full()
        .gap(8.0)
        .center_y()
        .child(icon(glyph, 14.0).color(tint).a11y_hidden())
        .child(
            text(message)
                .font(SMALL)
                .color(tint)
                .role("status")
                .wrap()
                .grow(1.0)
                .min_w(0.0),
        )
}

fn intro(p: Paint, message: &str) -> El {
    text(message.to_owned()).font(BODY).color(p.muted).wrap()
}

/// Puts the recovery code on the clipboard like **Copy export**: kept out of clipboard
/// history, and cleared after the clipboard time.
fn copy_code(s: &mut App, tx: &mut Tx) {
    let copied = match s.question.as_ref().map(|q| &q.ask) {
        Some(Ask::ShowRecoveryCode(code)) => {
            let _ = tx.copy_secret(code.as_str(), s.clipboard_time());
            true
        }
        _ => false,
    };
    if copied {
        let secs = s.settings.clipboard_secs;
        s.ok(tf!(
            "Copied. The clipboard clears in {} seconds.",
            "คัดลอกแล้ว คลิปบอร์ดจะถูกล้างใน {} วินาที",
            secs
        ));
    }
}

/// The dialog for the open cloud question, if there is one this task shows.
pub fn question_dialog(s: &App, p: Paint) -> Option<El> {
    let ask = &s.question.as_ref()?.ask;
    let f = &s.cloud_fields;
    let (a, b) = (f.first.text.as_str(), f.second.text.as_str());
    let ready = can_answer(ask, a, b);
    let (key, glyph, title, body, ok) = match ask {
        Ask::NewPassword => {
            let body = col()
                .w_full()
                .gap(12.0)
                .child(intro(
                    p,
                    t(
                        "Make a vault password. Keyman uses it to encrypt your secrets.",
                        "ตั้งรหัสผ่านตู้นิรภัย Keyman ใช้รหัสผ่านนี้เข้ารหัสความลับของคุณ",
                    ),
                ))
                .child(note(
                    "no-reset",
                    &ic::ALERT,
                    p.yellow,
                    t(
                        "Keyman cannot reset this password.",
                        "Keyman รีเซ็ตรหัสผ่านนี้ให้ไม่ได้",
                    )
                    .to_owned(),
                ))
                .child(field(
                    p,
                    "new-password",
                    t("Vault password", "รหัสผ่านตู้นิรภัย"),
                    &f.first,
                    false,
                    |s: &mut App| &mut s.cloud_fields.first,
                ))
                .child(field(
                    p,
                    "new-password-again",
                    t("Type the password again", "พิมพ์รหัสผ่านอีกครั้ง"),
                    &f.second,
                    false,
                    |s: &mut App| &mut s.cloud_fields.second,
                ))
                .children(
                    shown_password_error(a, b)
                        .map(|error| note("password-error", &ic::ALERT, p.red, error.into())),
                );
            (
                "new-password-dialog",
                &ic::LOCK,
                t("New vault password", "รหัสผ่านตู้นิรภัยใหม่"),
                body,
                t("OK", "ตกลง"),
            )
        }
        Ask::ShowRecoveryCode(code) => {
            let mut lines = col().key("recovery-code").gap(4.0).center();
            for (i, line) in code_lines(code).iter().enumerate() {
                lines = lines.child(
                    text(line.as_str())
                        .key(format!("code-{i}"))
                        .font(CODE)
                        .color(p.text),
                );
            }
            let wrong = !ready && code_chars(a).count() >= code_chars(code).count();
            let body = col()
                .w_full()
                .gap(12.0)
                .child(intro(
                    p,
                    t(
                        "Write down this code and keep it in a safe place. If you lose the password and this code, nobody can get your cloud data back.",
                        "จดรหัสนี้ไว้และเก็บในที่ปลอดภัย หากคุณลืมรหัสผ่านและทำรหัสนี้หาย จะไม่มีใครกู้ข้อมูลบนคลาวด์ของคุณคืนได้",
                    ),
                ))
                .child(
                    row()
                        .key("code-box")
                        .w_full()
                        .padding(14.0)
                        .gap(12.0)
                        .center_y()
                        .radius(10.0)
                        .fill(p.card)
                        .border(1.0, p.border)
                        .child(el().grow(1.0))
                        .child(lines)
                        .child(el().grow(1.0))
                        .child(button(
                            p,
                            "copy-code",
                            t("Copy", "คัดลอก"),
                            Some(&ic::COPY),
                            Kind::Secondary,
                            true,
                            copy_code,
                        )),
                )
                .child(field(
                    p,
                    "code-again",
                    t("Type the code again", "พิมพ์รหัสอีกครั้ง"),
                    &f.first,
                    true,
                    |s: &mut App| &mut s.cloud_fields.first,
                ))
                .children(wrong.then(|| {
                    note(
                        "code-error",
                        &ic::ALERT,
                        p.red,
                        t("The code is not the same.", "รหัสไม่ตรงกัน").into(),
                    )
                }));
            (
                "recovery-code-dialog",
                &ic::KEY,
                t("Recovery code", "รหัสกู้คืน"),
                body,
                t("Continue", "ต่อไป"),
            )
        }
        Ask::Password { tries_left } => {
            let body = col()
                .w_full()
                .gap(12.0)
                .child(intro(
                    p,
                    t(
                        "Type the vault password to open your cloud data.",
                        "พิมพ์รหัสผ่านตู้นิรภัยเพื่อเปิดข้อมูลบนคลาวด์",
                    ),
                ))
                .child(field(
                    p,
                    "password",
                    t("Vault password", "รหัสผ่านตู้นิรภัย"),
                    &f.first,
                    false,
                    |s: &mut App| &mut s.cloud_fields.first,
                ))
                // The bridge counts down after each wrong try.
                .children((*tries_left < cloud::PASSWORD_TRIES).then(|| {
                    note(
                        "tries-left",
                        &ic::ALERT,
                        p.red,
                        tf!(
                            "{} left",
                            "เหลืออีก {}",
                            count(usize::from(*tries_left), "try", "tries", "ครั้ง")
                        ),
                    )
                }));
            (
                "password-dialog",
                &ic::LOCK,
                t("Vault password", "รหัสผ่านตู้นิรภัย"),
                body,
                t("OK", "ตกลง"),
            )
        }
        Ask::RecoveryCode => {
            let body = col()
                .w_full()
                .gap(12.0)
                .child(intro(
                    p,
                    t(
                        "Type the recovery code that you wrote down when you made the vault password.",
                        "พิมพ์รหัสกู้คืนที่คุณจดไว้ตอนตั้งรหัสผ่านตู้นิรภัย",
                    ),
                ))
                .child(field(
                    p,
                    "recovery",
                    t("Recovery code", "รหัสกู้คืน"),
                    &f.first,
                    true,
                    |s: &mut App| &mut s.cloud_fields.first,
                ));
            (
                "recovery-dialog",
                &ic::KEY,
                t("Recovery code", "รหัสกู้คืน"),
                body,
                t("Continue", "ต่อไป"),
            )
        }
        Ask::Resolve { .. } => return None,
    };
    Some(
        col()
            .key(key)
            .role("dialog")
            .label(title)
            .w(460.0)
            .padding(22.0)
            .gap(14.0)
            .radius(14.0)
            .fill(p.bg)
            .border(1.0, p.border)
            .shadow(20.0, 60.0, Color::BLACK.alpha(0.4))
            .child(icon_tile(glyph, 40.0, p.accent, p.dark))
            .child(text(title.to_owned()).font(H2).color(p.text))
            .child(body)
            .child(
                row()
                    .w_full()
                    .gap(8.0)
                    .pt(6.0)
                    .child(el().grow(1.0))
                    // The only way out besides an answer: it stops the task.
                    .child(button(
                        p,
                        "question-cancel",
                        t("Cancel", "ยกเลิก"),
                        None,
                        Kind::Secondary,
                        true,
                        |s, _| s.cloud_cancel(),
                    ))
                    .child(button(
                        p,
                        "question-ok",
                        ok,
                        None,
                        Kind::Primary,
                        ready,
                        |s, _| s.cloud_answer(),
                    )),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_password_rules() {
        let short = new_password_error("short", "short").unwrap();
        assert!(
            [
                "The password must have 10 characters or more.",
                "รหัสผ่านต้องมีอย่างน้อย 10 ตัวอักษร"
            ]
            .contains(&short)
        );
        let differ = new_password_error("abcdefghij", "abcdefghik").unwrap();
        assert!(
            [
                "The two passwords are not the same.",
                "รหัสผ่านทั้งสองช่องไม่ตรงกัน"
            ]
            .contains(&differ)
        );
        assert_eq!(new_password_error("abcdefghij", "abcdefghij"), None);
        // Characters, not bytes: ten Thai letters are enough.
        let thai = "กขคงจฉชซฌญ";
        assert_eq!(new_password_error(thai, thai), None);
        assert!(new_password_error("กขคงจ", "กขคงจ").is_some());
    }

    #[test]
    fn first_use_card_rules() {
        let out = CloudState::SignedOut;
        let signed_in = CloudState::SignedIn {
            email: "a@b.c".into(),
        };
        for loaded in [false, true] {
            for cloud in [&out, &signed_in] {
                for done in [false, true] {
                    for dialog in [false, true] {
                        let want = loaded && *cloud == out && !done && !dialog;
                        assert_eq!(
                            show_first_use(loaded, cloud, done, dialog),
                            want,
                            "{loaded} {cloud:?} {done} {dialog}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn answers_are_complete_before_ok() {
        assert!(!can_answer(&Ask::NewPassword, "abcdefghij", "abcdefghik"));
        assert!(can_answer(&Ask::NewPassword, "abcdefghij", "abcdefghij"));
        assert!(!can_answer(&Ask::Password { tries_left: 3 }, "", ""));
        assert!(can_answer(&Ask::Password { tries_left: 1 }, "x", ""));
        assert!(!can_answer(&Ask::RecoveryCode, "", ""));
        let code = Ask::ShowRecoveryCode("ABCD-EFGH".to_owned().into());
        assert!(!can_answer(&code, "", ""));
        assert!(!can_answer(&code, "ABCD-EFGX", ""));
        assert!(can_answer(&code, " abcd efgh ", ""));
        assert!(!can_answer(
            &Ask::Resolve {
                project: "app".into(),
                conflicts: Vec::new()
            },
            "x",
            "x"
        ));
    }

    #[test]
    fn password_errors_wait_for_typing() {
        assert_eq!(shown_password_error("", ""), None);
        assert!(shown_password_error("short", "").is_some());
        assert_eq!(shown_password_error("abcdefghij", ""), None);
        assert!(shown_password_error("abcdefghij", "abc").is_some());
        assert_eq!(shown_password_error("abcdefghij", "abcdefghij"), None);
    }

    #[test]
    fn the_code_shows_in_two_lines() {
        let lines = code_lines("AAAA-BBBB-CCCC-DDDD-EEEE-FFFF-GGGG-HHHH");
        let lines: Vec<&str> = lines.iter().map(|line| line.as_str()).collect();
        assert_eq!(lines, ["AAAA-BBBB-CCCC-DDDD", "EEEE-FFFF-GGGG-HHHH"]);
    }

    #[test]
    fn the_code_check_matches_the_library() {
        use keyman_cli::cloud::crypto::normalize_recovery_code as library;
        let code = "7K2M-QD9T-4XWB-HN3R";
        for typed in [
            "7k2m qd9t 4xwb hn3r",
            "7K2M-QD9T-4XWB-HN3",
            "7K2MQD9T4XWBHN3R",
            "ß-ab",
            "",
        ] {
            assert_eq!(
                same_code(typed, code),
                library(typed) == library(code),
                "{typed}"
            );
            assert_eq!(
                code_chars(typed).collect::<String>(),
                library(typed),
                "{typed}"
            );
        }
    }
}
