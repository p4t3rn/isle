// SPDX-License-Identifier: GPL-3.0-or-later

//! Resolves an MPRIS player to the application that owns its session.

use std::path::Path;

use zbus::fdo::DBusProxy;
use zbus::names::{BusName, OwnedBusName};

use super::mpris::bounded;

pub(super) struct Source {
    pub identity: String,
    pub desktop_entry: String,
    pub browser: bool,
    pub rest_when_paused: bool,
}

pub(super) async fn identify(
    dbus: &DBusProxy<'_>,
    name: &OwnedBusName,
    identity: String,
    desktop_entry: String,
) -> Source {
    let desktop_entry = desktop_id(&desktop_entry).unwrap_or_default().to_owned();
    let browser = [name.as_str(), &identity, &desktop_entry]
        .into_iter()
        .any(is_browser);
    let generic_entry = is_generic(&desktop_entry);
    let mut source = Source {
        identity,
        desktop_entry,
        browser,
        rest_when_paused: browser,
    };
    if [name.as_str(), &source.identity, &source.desktop_entry]
        .into_iter()
        .any(is_qq_music_name)
    {
        source.use_qq_music(generic_entry);
        return source;
    }

    // Desktop players usually already tell us who they are. Chromium-based
    // shells may advertise the browser's name or omit their .desktop ID.
    if !browser && !generic_entry {
        return source;
    }
    if browser && !generic_entry {
        source.use_app_entry(None);
        return source;
    }
    let Ok(pid) = bounded(dbus.get_connection_unix_process_id(BusName::from(name))).await else {
        return source;
    };
    let Ok(exe) = tokio::fs::read_link(format!("/proc/{pid}/exe")).await else {
        return source;
    };
    let exe_name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if exe_name == "qqmusic" {
        source.use_qq_music(generic_entry);
    } else if is_electron(&exe, exe_name).await {
        let entry = desktop_id(exe_name).filter(|name| *name != "electron");
        source.use_app_entry(entry);
    } else if browser
        && is_browser(exe_name)
        && let Ok(args) = tokio::fs::read(format!("/proc/{pid}/cmdline")).await
        && let Some(app) = web_app(&args)
    {
        source.use_app_entry(app.desktop_entry.as_deref());
    }
    source
}

impl Source {
    fn use_qq_music(&mut self, generic_entry: bool) {
        self.identity = "QQ 音乐".into();
        if generic_entry {
            self.desktop_entry = "qqmusic".into();
        }
        self.browser = false;
        self.rest_when_paused = false;
    }

    fn use_app_entry(&mut self, entry: Option<&str>) {
        self.rest_when_paused = false;
        if let Some(entry) = entry {
            self.desktop_entry = entry.into();
        }
        if is_generic(&self.identity) && !is_generic(&self.desktop_entry) {
            self.identity.clone_from(&self.desktop_entry);
        }
    }
}

async fn is_electron(exe: &Path, name: &str) -> bool {
    if name == "electron" {
        return true;
    }
    let Some(dir) = exe.parent() else {
        return false;
    };
    // Electron's standard packaged and unpacked distribution layouts.
    tokio::fs::try_exists(dir.join("resources/app.asar"))
        .await
        .unwrap_or(false)
        || tokio::fs::try_exists(dir.join("resources/app/package.json"))
            .await
            .unwrap_or(false)
}

fn desktop_id(value: &str) -> Option<&str> {
    let value = value.strip_suffix(".desktop").unwrap_or(value);
    (!value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_')))
    .then_some(value)
}

#[derive(Debug, PartialEq, Eq)]
struct WebApp {
    desktop_entry: Option<String>,
}

/// Identifies an app from the bus owner's launch flags. A launcher that forwards
/// to an already running browser leaves no such flags on the owning process.
fn web_app(cmdline: &[u8]) -> Option<WebApp> {
    let mut argv = cmdline
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty());
    let executable_or_title = argv.next()?;
    let mut args: Vec<_> = argv
        .filter_map(|arg| std::str::from_utf8(arg).ok())
        .collect();
    let flattened = args.is_empty();
    if flattened {
        // Chromium on Linux replaces its argv with a single space-separated process title.
        args = std::str::from_utf8(executable_or_title)
            .ok()?
            .split(" --")
            .skip(1)
            .collect();
    }
    let mut args = args.into_iter().peekable();
    let mut class = None;
    let mut app_id = None;
    let mut profile = "Default";
    let mut app_url = false;
    let mut isolated = false;
    let mut no_remote = false;
    while let Some(arg) = args.next() {
        if arg == "--" || (flattened && (arg.is_empty() || arg.starts_with(' '))) {
            break;
        }
        let arg = if flattened {
            arg
        } else if let Some(flag) = arg.strip_prefix("--") {
            flag
        } else {
            continue;
        };
        match arg.split_once('=').unwrap_or((arg, "")) {
            ("class", "") if !flattened && args.peek().is_some_and(|arg| !arg.starts_with('-')) => {
                class = args.next();
            }
            ("class", value) => class = Some(value),
            ("app-id", value) => app_id = Some(value),
            ("profile-directory", value) => profile = value,
            ("app", value) => app_url = !value.is_empty(),
            ("user-data-dir", value) => isolated = !value.is_empty(),
            ("no-remote", _) => no_remote = true,
            _ => {}
        }
    }
    let class = class
        .and_then(desktop_id)
        .filter(|id| id.starts_with("WebApp-"));
    // Linux Mint Web App Manager launches isolated Chromium or Firefox apps this way.
    if let Some(class) = class.filter(|_| (isolated && app_url) || no_remote) {
        return Some(WebApp {
            desktop_entry: Some(class.into()),
        });
    }
    if !isolated {
        return None;
    }
    if let Some(id) = app_id.filter(|id| is_chromium_app_id(id)) {
        let profile = profile.replace(' ', "_");
        return Some(WebApp {
            desktop_entry: desktop_id(&profile).map(|profile| format!("chrome-{id}-{profile}")),
        });
    }
    app_url.then_some(WebApp {
        desktop_entry: None,
    })
}

fn is_chromium_app_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}

const BROWSERS: &[&str] = &[
    "firefox",
    "zen",
    "chrom",
    "brave",
    "edge",
    "vivaldi",
    "opera",
    "epiphany",
    "librewolf",
    "floorp",
    "waterfox",
    "mullvad",
    "icecat",
];

fn is_browser(name: &str) -> bool {
    let name = name.to_lowercase();
    BROWSERS.iter().any(|browser| name.contains(browser))
}

fn is_generic(name: &str) -> bool {
    let web_app = name.starts_with("WebApp-")
        || name
            .strip_prefix("chrome-")
            .and_then(|name| name.split_once('-'))
            .is_some_and(|(id, _)| is_chromium_app_id(id));
    !web_app && (name.is_empty() || is_browser(name) || name.eq_ignore_ascii_case("electron"))
}

fn is_qq_music_name(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    let name = name
        .strip_prefix("org.mpris.mediaplayer2.")
        .unwrap_or(&name);
    let name = name.strip_suffix(".desktop").unwrap_or(name);
    let name = name.split_once(".instance").map_or(name, |(id, _)| id);
    matches!(
        name,
        "qqmusic" | "qq music" | "qq音乐" | "qq 音乐" | "com.qq.qqmusic"
    )
}

#[cfg(test)]
mod tests {
    use super::{Source, WebApp, desktop_id, is_browser, is_qq_music_name, web_app};

    #[test]
    fn recognises_qq_music_without_taking_every_chromium_session() {
        for name in [
            "QQMusic",
            "QQ 音乐",
            "org.mpris.MediaPlayer2.qqmusic.instance1",
            "com.qq.QQmusic.desktop",
        ] {
            assert!(is_qq_music_name(name), "{name}");
        }
        for name in ["Chromium", "netease-cloud-music", "not-qqmusic"] {
            assert!(!is_qq_music_name(name), "{name}");
        }
        assert!(is_browser("org.mpris.MediaPlayer2.chromium.instance1"));
        assert!(is_browser("org.mpris.MediaPlayer2.firefox.instance_1_7"));
        assert!(!is_browser("Spotify"));
    }

    #[test]
    fn keeps_app_identity_and_chromium_timing_without_browser_resting() {
        let mut source = Source {
            identity: "Chromium".into(),
            desktop_entry: "chromium".into(),
            browser: true,
            rest_when_paused: true,
        };
        source.use_app_entry(Some("WebApp-网易云音乐1234"));
        assert_eq!(source.identity, "WebApp-网易云音乐1234");
        assert_eq!(source.desktop_entry, source.identity);
        assert!(source.browser);
        assert!(!source.rest_when_paused);
        assert_eq!(desktop_id("app.example.desktop"), Some("app.example"));
        assert_eq!(desktop_id("../example.desktop"), None);
    }

    #[test]
    fn web_app_source_requires_its_own_profile() {
        assert!(
            web_app(b"chrome\0--app=https://example.org\0--class=WebApp-Music0001\0").is_none()
        );
        assert!(web_app(b"chrome\0--user-data-dir=/tmp/app\0https://example.org\0").is_none());
        assert_eq!(
            web_app(b"chrome\0--user-data-dir=/tmp/app\0--app=https://example.org\0"),
            Some(WebApp {
                desktop_entry: None
            })
        );
        for (args, expected) in [
            (
                "/opt/google/chrome/chrome --user-data-dir=/tmp/app --class=WebApp-音乐0001 --app=https://example.org\0\0",
                "WebApp-音乐0001",
            ),
            (
                "chrome\0--user-data-dir=/tmp/app\0--app=https://example.org\0--class=WebApp-音乐0001\0",
                "WebApp-音乐0001",
            ),
            (
                "firefox\0--no-remote\0--class\0WebApp-Music0001\0",
                "WebApp-Music0001",
            ),
            (
                "chrome\0--user-data-dir=/tmp/app\0--app-id=abcdefghijklmnopabcdefghijklmnop\0--profile-directory=Profile 3\0",
                "chrome-abcdefghijklmnopabcdefghijklmnop-Profile_3",
            ),
        ] {
            assert_eq!(
                web_app(args.as_bytes()).unwrap().desktop_entry.as_deref(),
                Some(expected)
            );
        }
    }
}
