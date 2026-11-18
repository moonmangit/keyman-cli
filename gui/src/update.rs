//! Updates (Settings › About): find a newer release of the desktop app on GitHub and put it in
//! place with one click. At start, the app also checks once a day in the background, when the
//! setting is on. It never installs without a click.
//!
//! Installing downloads this system's zip from the release, checks its SHA-256 against the
//! release's `SHA256SUMS.txt`, unpacks it with the system's own tools, and puts it in place of
//! this copy. On Windows the running .exe is renamed aside (Windows allows that) and the new one
//! is copied to its place; on macOS the whole Keyman.app is replaced. If a step fails, the old
//! copy goes back. The new version runs after a restart; the copy set aside is deleted at the
//! next start.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::App;
use crate::i18n::t;

/// The repository that the releases come from.
const REPO: &str = "moonmangit/keyman-cli";
/// The checksums file of each release.
const SUMS: &str = "SHA256SUMS.txt";
/// The largest download that the app accepts.
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
/// The automatic check runs at most once in this time.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The state of the update.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Update {
    #[default]
    NotChecked,
    Checking,
    /// This build is the newest release (or newer).
    UpToDate,
    /// A newer release.
    Available(Release),
    /// Downloading, checking and putting it in place.
    Installing {
        version: String,
    },
    /// In place: the next start runs it.
    Installed {
        version: String,
    },
    Failed(String),
}

/// A release that is newer than this build.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release page.
    pub page: String,
    /// This system's zip and the checksums file, when the release has both.
    pub files: Option<(String, String)>,
}

/// "v1.2.3" or "1.2.3" → (1, 2, 3). Text after a '-' or '+' is ignored.
pub fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
        parts.next().unwrap_or(Some(0))?,
    ))
}

/// This system's zip in the releases, if the releases have one.
pub fn asset() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("Keyman-windows-x64.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("Keyman-macos-arm64.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("Keyman-macos-x64.zip")
    } else {
        None
    }
}

/// Only files from this repository's releases are downloaded.
fn trusted(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/"))
}

/// Reads GitHub's answer for the newest release.
pub fn verdict(json: &serde_json::Value) -> Result<Update, String> {
    let tag = json["tag_name"].as_str().ok_or_else(|| {
        t(
            "GitHub sent an answer that Keyman cannot read.",
            "GitHub ตอบกลับในรูปแบบที่ Keyman อ่านไม่ได้",
        )
        .to_owned()
    })?;
    let (Some(latest), Some(mine)) = (parse(tag), parse(current())) else {
        return Err(tf!(
            "Keyman cannot read the version \"{}\".",
            "Keyman อ่านเวอร์ชัน \"{}\" ไม่ได้",
            tag
        ));
    };
    if latest <= mine {
        return Ok(Update::UpToDate);
    }
    let url_of = |name: &str| {
        json["assets"].as_array()?.iter().find_map(|a| {
            let url = a["browser_download_url"].as_str()?;
            (a["name"].as_str() == Some(name) && trusted(url)).then(|| url.to_owned())
        })
    };
    let files = asset().and_then(|zip| Some((url_of(zip)?, url_of(SUMS)?)));
    Ok(Update::Available(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        page: json["html_url"]
            .as_str()
            .unwrap_or(crate::REPO_URL)
            .to_owned(),
        files,
    }))
}

/// Asks GitHub for the newest release (not a draft or a pre-release).
fn check() -> Result<Update, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = ureq::get(&url)
        .header("User-Agent", "keyman-app")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(404) => {
                t("No release is available yet.", "ยังไม่มีรุ่นที่เผยแพร่").to_owned()
            }
            error => tf!(
                "Keyman cannot connect to GitHub: {}",
                "Keyman เชื่อมต่อ GitHub ไม่ได้: {}",
                error
            ),
        })?;
    let json: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|error| error.to_string())?;
    verdict(&json)
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let mut response = ureq::get(url)
        .header("User-Agent", "keyman-app")
        .call()
        .map_err(|error| tf!("The download failed: {}", "ดาวน์โหลดไม่สำเร็จ: {}", error))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .read_to_vec()
        .map_err(|error| tf!("The download failed: {}", "ดาวน์โหลดไม่สำเร็จ: {}", error))
}

/// The file's SHA-256 must agree with its line in SHA256SUMS.txt.
pub fn verify(name: &str, bytes: &[u8], sums: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let want = sums
        .lines()
        .find_map(|line| {
            let (hash, file) = line.trim().split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
        })
        .ok_or_else(|| {
            tf!(
                "The checksums of the release do not include {}.",
                "ไฟล์ checksum ของรุ่นนี้ไม่มี {}",
                name
            )
        })?;
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got == want {
        Ok(())
    } else {
        Err(t(
            "The download does not agree with its checksum. Keyman did not install it.",
            "ไฟล์ที่ดาวน์โหลดไม่ตรงกับ checksum Keyman จึงไม่ติดตั้ง",
        )
        .to_owned())
    }
}

/// Why this copy cannot update itself, if it cannot. The page then offers the release page.
pub fn cannot_install(release: &Release) -> Option<String> {
    if asset().is_none() || release.files.is_none() {
        return Some(
            t(
                "This release has no file for this system. Download it from the release page.",
                "รุ่นนี้ไม่มีไฟล์สำหรับระบบนี้ ดาวน์โหลดจากหน้ารุ่น",
            )
            .to_owned(),
        );
    }
    target().err()
}

/// What an update replaces: this .exe, or on macOS the Keyman.app that it runs in.
fn target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        return Err(t(
            "This is a development build (it runs from target/). Update it from the source code.",
            "นี่คือบิลด์สำหรับพัฒนา (รันจาก target/) อัปเดตจากซอร์สโค้ด",
        )
        .to_owned());
    }
    if cfg!(target_os = "macos")
        && let Some(app) = exe
            .ancestors()
            .nth(3)
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
    {
        return Ok(app.to_path_buf());
    }
    Ok(exe)
}

/// Where the replaced copy stays until the next start.
fn set_aside(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.strip_suffix(".exe") {
        Some(stem) => target.with_file_name(format!("{stem}.old.exe")),
        None => target.with_file_name(format!("{name}.old")),
    }
}

/// Downloads, checks and puts `release` in place.
fn install(release: &Release) -> Result<(), String> {
    let asset = asset().ok_or("no file for this system")?;
    let (zip_url, sums_url) = release.files.clone().ok_or("no file for this system")?;
    let target = target()?;
    let work = std::env::temp_dir().join(format!(
        "keyman-update-{}",
        release.version.replace(['/', '\\', '.'], "_")
    ));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let result = (|| {
        let sums = String::from_utf8(download(&sums_url)?).map_err(|error| error.to_string())?;
        let bytes = download(&zip_url)?;
        verify(asset, &bytes, &sums)?;
        let zip = work.join(asset);
        std::fs::write(&zip, &bytes).map_err(|error| error.to_string())?;
        let unpacked = work.join("unpacked");
        unzip(&zip, &unpacked)?;
        replace(&target, &unpacked)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Runs a system tool without a console window and returns its error text when it fails.
fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if text.is_empty() {
            format!("{program} stopped with {}", output.status)
        } else {
            text
        })
    }
}

fn unzip(zip: &Path, out: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let q = |p: &Path| p.to_string_lossy().replace('\'', "''");
        let command = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            q(zip),
            q(out)
        );
        run(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", &command],
        )
    }
    #[cfg(not(windows))]
    {
        run(
            "ditto",
            &["-x", "-k", &zip.to_string_lossy(), &out.to_string_lossy()],
        )
    }
}

/// Puts the unpacked new copy in place of `target`, and keeps the old one aside (it goes back
/// if a step fails).
fn replace(target: &Path, unpacked: &Path) -> Result<(), String> {
    let new = if target.extension().is_some_and(|e| e == "app") {
        unpacked.join("Keyman.app")
    } else if cfg!(windows) {
        unpacked.join("Keyman.exe")
    } else {
        unpacked
            .join("Keyman.app")
            .join("Contents")
            .join("MacOS")
            .join("Keyman")
    };
    if !new.exists() {
        return Err(t(
            "The download does not contain Keyman.",
            "ไฟล์ที่ดาวน์โหลดไม่มี Keyman",
        )
        .to_owned());
    }
    let old = set_aside(target);
    remove(&old);
    match std::fs::rename(target, &old) {
        Ok(()) => {}
        // Installed where this account cannot write (Program Files for all users, or
        // /Applications for a standard Mac account): the system gives the permission.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return swap_as_admin(target, &new, &old);
        }
        Err(error) => {
            return Err(tf!(
                "Keyman cannot move the current version aside: {}",
                "Keyman ย้ายเวอร์ชันปัจจุบันออกไม่ได้: {}",
                error
            ));
        }
    }
    if let Err(error) = copy_into_place(&new, target) {
        remove(target);
        let _ = std::fs::rename(&old, target);
        return Err(tf!(
            "Keyman cannot put the new version in place: {}",
            "Keyman วางเวอร์ชันใหม่ไม่ได้: {}",
            error
        ));
    }
    Ok(())
}

fn admin_refused() -> String {
    t(
        "Keyman is installed for all users. The update needs administrator permission, and the permission was not given.",
        "Keyman ติดตั้งไว้สำหรับผู้ใช้ทุกคน การอัปเดตต้องได้สิทธิ์ผู้ดูแลระบบ และไม่ได้รับสิทธิ์นั้น",
    )
    .to_owned()
}

/// The same swap as `replace`, with administrator rights. Windows shows its permission prompt
/// (UAC); macOS asks for an administrator's password.
#[cfg(windows)]
fn swap_as_admin(target: &Path, new: &Path, old: &Path) -> Result<(), String> {
    let script = elevated_swap_script(target, new, old);
    // An outer PowerShell starts the elevated one and waits for it. The swap goes encoded, so
    // no path needs quotes on a command line.
    let outer = format!(
        "$p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{}'; exit $p.ExitCode",
        encode_command(&script)
    );
    match run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &outer],
    ) {
        Ok(()) => Ok(()),
        Err(error) if error.to_lowercase().contains("cancel") => Err(admin_refused()),
        Err(error) => Err(tf!(
            "Keyman cannot replace itself, also with administrator permission: {}",
            "Keyman แทนที่ตัวเองไม่ได้ แม้มีสิทธิ์ผู้ดูแลระบบ: {}",
            error
        )),
    }
}

/// PowerShell that puts `new` in place of `target`, keeps `target` aside as `old`, and puts it
/// back if the copy fails. Exit 0 when it works.
#[cfg(windows)]
fn elevated_swap_script(target: &Path, new: &Path, old: &Path) -> String {
    let q = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let (t, n, o) = (q(target), q(new), q(old));
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         try {{\n\
           if (Test-Path -LiteralPath '{o}') {{ Remove-Item -LiteralPath '{o}' -Force }}\n\
           Move-Item -LiteralPath '{t}' -Destination '{o}'\n\
           try {{ Copy-Item -LiteralPath '{n}' -Destination '{t}' }}\n\
           catch {{ Move-Item -LiteralPath '{o}' -Destination '{t}' -Force; exit 2 }}\n\
           exit 0\n\
         }} catch {{ exit 1 }}\n"
    )
}

/// PowerShell's -EncodedCommand: base64 of the script in UTF-16LE.
#[cfg(windows)]
fn encode_command(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(not(windows))]
fn swap_as_admin(target: &Path, new: &Path, old: &Path) -> Result<(), String> {
    // A shell command that runs with an administrator's password through AppleScript.
    let sh = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
    let (t, n, o) = (sh(target), sh(new), sh(old));
    let command = format!(
        "rm -rf {o}; mv {t} {o} && (ditto {n} {t} || (rm -rf {t}; mv {o} {t}; exit 2)) && touch {t}"
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    match run("osascript", &["-e", &script]) {
        Ok(()) => Ok(()),
        Err(error) if error.contains("-128") || error.to_lowercase().contains("cancel") => {
            Err(admin_refused())
        }
        Err(error) => Err(tf!(
            "Keyman cannot replace itself, also with administrator permission: {}",
            "Keyman แทนที่ตัวเองไม่ได้ แม้มีสิทธิ์ผู้ดูแลระบบ: {}",
            error
        )),
    }
}

fn copy_into_place(new: &Path, target: &Path) -> Result<(), String> {
    if new.is_dir() {
        // An .app bundle: ditto copies it whole (signature, permissions, icon).
        run(
            "ditto",
            &[&new.to_string_lossy(), &target.to_string_lossy()],
        )?;
        // Finder and the Dock keep app icons by path: a newer date makes them read the new icon.
        let _ = run("touch", &[&target.to_string_lossy()]);
        Ok(())
    } else {
        std::fs::copy(new, target)
            .map(|_| ())
            .map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755));
        }
        Ok(())
    }
}

fn remove(p: &Path) {
    if p.is_dir() {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// At start: deletes the copy that an update set aside.
pub fn clean_up_previous() {
    if let Ok(target) = target() {
        remove(&set_aside(&target));
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The automatic check is due: the setting is on and the last check is a day old.
pub fn check_due(on: bool, last_check: u64, now: u64) -> bool {
    on && now.saturating_sub(last_check) >= CHECK_EVERY.as_secs()
}

impl App {
    /// Runs `work` on its own thread and gives its result to `done` on the UI thread.
    fn in_background<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut App, T) + Send + 'static,
    ) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let _ = rune::task::spawn("keyman-update", move || {
            let result = work();
            handle.update(move |s, _| done(s, result));
        });
    }

    /// **Check for updates**.
    pub fn check_for_updates(&mut self) {
        self.start_check(false);
    }

    /// At start, when the check is due: a newer version shows a message, and an error says
    /// nothing.
    pub fn check_for_updates_at_start(&mut self) {
        if check_due(
            self.settings.update_check,
            self.settings.last_update_check,
            now_secs(),
        ) {
            self.start_check(true);
        }
    }

    fn start_check(&mut self, quiet: bool) {
        if matches!(self.update, Update::Checking | Update::Installing { .. }) {
            return;
        }
        self.update = Update::Checking;
        self.in_background(check, move |s, result| match result {
            Ok(update) => {
                s.update_settings(|st| st.last_update_check = now_secs());
                if quiet && let Update::Available(release) = &update {
                    s.show_toast(
                        true,
                        tf!(
                            "Keyman {} is available. Open Settings › About to update.",
                            "Keyman {} พร้อมให้อัปเดตแล้ว เปิด การตั้งค่า › เกี่ยวกับ เพื่ออัปเดต",
                            release.version
                        ),
                    );
                }
                s.update = update;
            }
            Err(_) if quiet => s.update = Update::NotChecked,
            Err(error) => s.update = Update::Failed(error),
        });
    }

    /// **Update now**: downloads and puts the available version in place.
    pub fn install_update(&mut self) {
        let Update::Available(release) = self.update.clone() else {
            return;
        };
        let version = release.version.clone();
        self.update = Update::Installing {
            version: version.clone(),
        };
        self.in_background(
            move || install(&release),
            move |s, result| {
                s.update = match result {
                    Ok(()) => Update::Installed { version },
                    Err(error) => Update::Failed(error),
                };
            },
        );
    }

    /// **Restart now**: starts the installed version and closes this one.
    pub fn restart_after_update(&mut self) {
        let Ok(target) = target() else {
            return;
        };
        let started = if target.extension().is_some_and(|e| e == "app") {
            std::process::Command::new("open")
                .arg("-n")
                .arg(&target)
                .spawn()
        } else {
            std::process::Command::new(&target).spawn()
        };
        match started {
            Ok(_) => std::process::exit(0),
            Err(error) => self.fail(tf!(
                "Keyman cannot start the new version: {}",
                "Keyman เปิดเวอร์ชันใหม่ไม่ได้: {}",
                error
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_versions() {
        assert_eq!(parse("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse("1.2"), Some((1, 2, 0)));
        assert_eq!(parse("v2.0.1-beta.1"), Some((2, 0, 1)));
        assert_eq!(parse("latest"), None);
    }

    fn release_json(tag: &str, assets: &[(&str, &str)]) -> serde_json::Value {
        json!({
            "tag_name": tag,
            "html_url": "https://github.com/moonmangit/keyman-cli/releases/tag/v9.0.0",
            "assets": assets
                .iter()
                .map(|(name, url)| json!({"name": name, "browser_download_url": url}))
                .collect::<Vec<_>>(),
        })
    }

    #[test]
    fn newer_only_when_greater() {
        let same = release_json(&format!("v{}", current()), &[]);
        assert_eq!(verdict(&same), Ok(Update::UpToDate));
        assert_eq!(verdict(&release_json("v0.0.1", &[])), Ok(Update::UpToDate));
        assert!(verdict(&release_json("nightly", &[])).is_err());
        assert!(verdict(&json!({"message": "Not Found"})).is_err());
        let Ok(Update::Available(release)) = verdict(&release_json("v999.0.0", &[])) else {
            panic!("a greater version must be available");
        };
        assert_eq!(release.version, "999.0.0");
        // No files: the page offers the release page in place of Update now.
        assert_eq!(release.files, None);
        assert!(cannot_install(&release).is_some());
    }

    #[test]
    fn takes_only_this_repositorys_files() {
        let Some(zip) = asset() else { return };
        let base = "https://github.com/moonmangit/keyman-cli/releases/download/v999.0.0";
        let good = release_json(
            "v999.0.0",
            &[
                (zip, &format!("{base}/{zip}")),
                (SUMS, &format!("{base}/{SUMS}")),
            ],
        );
        let Ok(Update::Available(release)) = verdict(&good) else {
            panic!("must be available");
        };
        assert_eq!(
            release.files,
            Some((format!("{base}/{zip}"), format!("{base}/{SUMS}")))
        );
        let other = release_json(
            "v999.0.0",
            &[
                (zip, "https://example.test/keyman.zip"),
                (SUMS, &format!("{base}/{SUMS}")),
            ],
        );
        let Ok(Update::Available(release)) = verdict(&other) else {
            panic!("must be available");
        };
        assert_eq!(release.files, None);
    }

    #[test]
    fn checksums_must_agree() {
        // sha256("hello")
        let sums = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  Keyman-windows-x64.zip\nabc  other.zip\n";
        assert!(verify("Keyman-windows-x64.zip", b"hello", sums).is_ok());
        assert!(verify("Keyman-windows-x64.zip", b"tampered", sums).is_err());
        assert!(verify("missing.zip", b"hello", sums).is_err());
    }

    #[test]
    fn old_copy_names() {
        assert_eq!(
            set_aside(Path::new("C:/apps/Keyman.exe")),
            Path::new("C:/apps/Keyman.old.exe")
        );
        assert_eq!(
            set_aside(Path::new("/Applications/Keyman.app")),
            Path::new("/Applications/Keyman.app.old")
        );
    }

    #[test]
    fn checks_once_a_day_when_on() {
        let day = CHECK_EVERY.as_secs();
        assert!(check_due(true, 0, day));
        assert!(!check_due(true, day, day + 60));
        assert!(check_due(true, day, 2 * day));
        assert!(!check_due(false, 0, 10 * day));
        // A clock that goes back does not check again and again.
        assert!(!check_due(true, 5 * day, day));
    }

    /// Asks the real GitHub API: `cargo test -- --ignored github_answers`.
    #[test]
    #[ignore]
    fn github_answers() {
        assert!(check().is_ok(), "{:?}", check());
    }

    #[test]
    fn dev_builds_do_not_replace_themselves() {
        // Tests run from target/, so this is a development build.
        assert!(target().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn encodes_like_powershell() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes("exit 0"))
        assert_eq!(encode_command("exit 0"), "ZQB4AGkAdAAgADAA");
    }

    /// The elevated swap's script, run here without elevation on test files.
    #[cfg(windows)]
    #[test]
    fn elevated_swap_script_swaps_and_keeps_the_old_copy() {
        let dir = std::env::temp_dir().join(format!("keyman-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("it's new")).unwrap();
        let (target, new, old) = (
            dir.join("Keyman.exe"),
            dir.join("it's new").join("Keyman.exe"),
            dir.join("Keyman.old.exe"),
        );
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        let script = elevated_swap_script(&target, &new, &old);
        run(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encode_command(&script),
            ],
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
