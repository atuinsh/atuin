//! Finds running browsers and Electron applications through `/proc`.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;

use memchr::memmem;

pub struct Process {
    pid: u32,
    parent: u32,
    exe: PathBuf,
    /// The arguments, separated by NUL bytes.
    cmdline: Vec<u8>,
    sandbox: Sandbox,
}

/// The sandbox that a process runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    None,
    /// Flatpak puts `/.flatpak-info` into the root of every sandbox. Its D-Bus proxy keeps the
    /// application from reading whether an accessibility client is connected.
    Flatpak {
        /// The instance that the sandbox belongs to. The application and the D-Bus proxy that
        /// speaks for it on the bus run in two sandboxes of the same instance.
        instance: Option<u64>,
    },
}

/// Lists the processes of the current user.
pub fn own_processes() -> impl Iterator<Item = Process> {
    let own_uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    std::fs::read_dir("/proc").into_iter().flatten().filter_map(move |entry| {
        let entry = entry.ok()?;
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let mut path = entry.path();
        if Some(std::fs::metadata(&path).ok()?.uid()) != own_uid {
            return None;
        }
        path.push("exe");
        let exe = std::fs::read_link(&path).ok()?;
        path.set_file_name("cmdline");
        let cmdline = std::fs::read(&path).ok()?;
        path.set_file_name("stat");
        let parent = parent_of(&std::fs::read_to_string(&path).ok()?)?;
        path.set_file_name("root");
        path.push(".flatpak-info");
        let sandbox = match std::fs::read_to_string(&path) {
            Ok(info) => Sandbox::Flatpak {
                instance: flatpak_instance(&info),
            },
            Err(_) => Sandbox::None,
        };
        Some(Process {
            pid,
            parent,
            exe,
            cmdline,
            sandbox,
        })
    })
}

/// The two paths of a file beside the executable of a process.
pub struct BesideExe {
    /// The path through `/proc/<pid>/root`, which is the process's own view of the file
    /// system.
    pub own_view: PathBuf,
    /// The path in the helper's view of the file system.
    pub host_view: PathBuf,
}

/// The paths of the file `name` beside the executable of the process `pid`. Returns `None`
/// when the executable can't be read.
pub fn beside_exe(pid: u32, name: &str) -> Option<BesideExe> {
    let mut process = PathBuf::from(format!("/proc/{pid}/exe"));
    let mut host_view = std::fs::read_link(&process).ok()?;
    host_view.set_file_name(name);
    process.set_file_name("root");
    process.push(host_view.strip_prefix("/").ok()?);
    Some(BesideExe {
        own_view: process,
        host_view,
    })
}

/// Reads the instance id from the contents of `/.flatpak-info`.
fn flatpak_instance(info: &str) -> Option<u64> {
    info.lines().find_map(|line| line.strip_prefix("instance-id=")?.trim().parse().ok())
}

/// Whether the process `pid` runs the application whose app id is `id`.
///
/// A Flatpak sandbox names its application in `/.flatpak-info`. Desktops start other
/// applications in a systemd unit that they name after the application's desktop entry, such as
/// `app-gnome-org.gnome.Nautilus-1234.scope`.
pub fn runs_app_id(pid: u32, id: &str) -> bool {
    if let Ok(info) = std::fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")) {
        return flatpak_app(&info) == Some(id);
    }
    std::fs::read_to_string(format!("/proc/{pid}/cgroup")).is_ok_and(|cgroups| {
        cgroups
            .lines()
            .any(|line| line.rsplit('/').next().is_some_and(|unit| unit_runs_app_id(unit, id)))
    })
}

/// Reads the app id of the application from the contents of `/.flatpak-info`.
fn flatpak_app(info: &str) -> Option<&str> {
    let mut in_application = false;
    for line in info.lines() {
        if line.starts_with('[') {
            in_application = line == "[Application]";
        } else if in_application && let Some(name) = line.strip_prefix("name=") {
            return Some(name.trim());
        }
    }
    None
}

/// Whether the systemd unit `unit` runs the application whose app id is `id`.
///
/// systemd's convention names an application's unit `app-[<launcher>-]<id>-<random>.scope`
/// or `app-[<launcher>-]<id>[@<random>].service`, with each `-` of the id escaped as `\x2d`.
fn unit_runs_app_id(unit: &str, id: &str) -> bool {
    let Some(rest) = unit.strip_prefix("app-") else {
        return false;
    };
    let escaped = id.replace('-', "\\x2d");
    rest.match_indices(&escaped).any(|(at, _)| {
        let after = &rest[at + escaped.len()..];
        (at == 0 || rest.as_bytes()[at - 1] == b'-')
            && (after.starts_with(['-', '@']) || after == ".service" || after == ".scope")
    })
}

/// The Flatpak instance that the process `pid` runs in, if it runs in one.
pub fn instance_of(processes: &[Process], pid: u32) -> Option<u64> {
    match processes.iter().find(|process| process.pid == pid)?.sandbox {
        Sandbox::Flatpak { instance } => instance,
        Sandbox::None => None,
    }
}

/// Reads the parent's pid from the contents of `/proc/<pid>/stat`. The command name in
/// parentheses can hold spaces and parentheses, so the fields count from the last `)`.
fn parent_of(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// The main process of a browser or an Electron application.
#[derive(Debug, PartialEq, Eq)]
pub struct Browser {
    pub pid: u32,
    pub name: String,
    pub sandbox: Sandbox,
}

/// How many parents a walk up the process tree passes at most.
const MAX_ANCESTORS: usize = 64;

/// The arguments that mark a helper process of a browser: Chromium's and Electron's, then
/// Gecko's.
const HELPER_ARGUMENTS: [&[u8]; 2] = [b"--type=", b"-contentproc"];

/// Finds the main process of every browser and Electron application.
///
/// A main process is recognized by its helper processes, which run the same executable below
/// the main process. Chromium and Electron pass a helper a `--type=` argument, and Gecko passes
/// a helper `-contentproc`. A launcher that wraps an application can also look like a main
/// process, because some copies of the launcher run the helpers and so carry the helpers'
/// arguments. For example, Flatpak's `bwrap` does this. Of two processes found where one runs
/// inside the other, only the inner one is the application.
pub fn find_browsers(processes: &[Process]) -> Vec<Browser> {
    let parents: HashMap<u32, u32> = processes.iter().map(|p| (p.pid, p.parent)).collect();
    let descends_from = |pid: u32, ancestor: u32| {
        let mut current = pid;
        for _ in 0..MAX_ANCESTORS {
            match parents.get(&current) {
                Some(&parent) if parent == ancestor => return true,
                Some(&parent) if parent != current => current = parent,
                _ => return false,
            }
        }
        false
    };
    let is_helper = |process: &Process| {
        HELPER_ARGUMENTS.iter().any(|argument| memmem::find(&process.cmdline, argument).is_some())
    };
    let has_helpers = |main: &Process| {
        processes
            .iter()
            .any(|p| p.exe == main.exe && is_helper(p) && descends_from(p.pid, main.pid))
    };
    let found: Vec<&Process> =
        processes.iter().filter(|process| !is_helper(process) && has_helpers(process)).collect();
    found
        .iter()
        .filter(|outer| !found.iter().any(|inner| descends_from(inner.pid, outer.pid)))
        .filter_map(|process| {
            Some(Browser {
                pid: process.pid,
                name: process.exe.file_name()?.to_str()?.to_owned(),
                sandbox: process.sandbox,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::flatpak(
        "app-flatpak-md.obsidian.Obsidian-4165938694.scope",
        "md.obsidian.Obsidian",
        true
    )]
    #[case::gnome("app-gnome-org.gnome.Nautilus-1234.scope", "org.gnome.Nautilus", true)]
    #[case::kde_service("app-org.kde.dolphin@0a1b2c.service", "org.kde.dolphin", true)]
    #[case::plain_service("app-org.kde.dolphin.service", "org.kde.dolphin", true)]
    #[case::escaped_hyphen("app-gnome-org.example.my\\x2dapp-77.scope", "org.example.my-app", true)]
    #[case::longer_id("app-gnome-org.gnome.Nautilus.Preview-1.scope", "org.gnome.Nautilus", false)]
    #[case::id_inside_another("app-gnome-xorg.gnome.Nautilus-1.scope", "org.gnome.Nautilus", false)]
    #[case::not_an_app("session-2.scope", "org.gnome.Nautilus", false)]
    #[case::other_app("app-gnome-org.gnome.Calculator-9.scope", "org.gnome.Nautilus", false)]
    fn units_run_the_app_ids_that_they_name(
        #[case] unit: &str,
        #[case] id: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(unit_runs_app_id(unit, id), expected);
    }

    #[rstest]
    fn flatpak_info_names_its_application() {
        let info = "[Application]\nname=md.obsidian.Obsidian\nruntime=runtime/x\n\n[Instance]\\
                    nname=other\ninstance-id=4165938694\n";
        assert_eq!(flatpak_app(info), Some("md.obsidian.Obsidian"));
        assert_eq!(flatpak_app("[Instance]\nname=other\n"), None);
    }

    fn process(pid: u32, parent: u32, exe: &str, args: &[&str]) -> Process {
        Process {
            pid,
            parent,
            exe: exe.into(),
            cmdline: args.join("\0").into_bytes(),
            sandbox: Sandbox::None,
        }
    }

    fn found(pid: u32, name: &str) -> Browser {
        Browser {
            pid,
            name: name.to_owned(),
            sandbox: Sandbox::None,
        }
    }

    #[rstest]
    fn main_processes_are_found_and_helpers_are_skipped() {
        let processes = [
            process(1, 0, "/usr/lib/firefox-esr/firefox-esr", &["firefox-esr"]),
            process(2, 1, "/usr/lib/firefox-esr/firefox-esr", &[
                "firefox-esr",
                "-contentproc",
                "tab",
            ]),
            process(3, 0, "/usr/lib/chromium/chromium", &["chromium", "--no-first-run"]),
            process(4, 3, "/usr/lib/chromium/chromium", &["chromium", "--type=renderer"]),
            process(5, 0, "/opt/code/code", &["code", "--no-sandbox"]),
            process(6, 5, "/opt/code/code", &["code", "--type=zygote"]),
            process(7, 0, "/usr/bin/gedit", &["gedit", "notes.txt"]),
            process(8, 0, "/usr/lib/librewolf/librewolf", &["librewolf"]),
            process(9, 8, "/usr/lib/librewolf/librewolf", &["librewolf", "-contentproc", "tab"]),
            // A program that is named like Firefox but doesn't run helpers.
            process(10, 0, "/usr/bin/firefox-sync-tool", &["firefox-sync-tool"]),
        ];
        assert_eq!(find_browsers(&processes), [
            found(1, "firefox-esr"),
            found(3, "chromium"),
            found(5, "code"),
            found(8, "librewolf"),
        ]);
    }

    /// The process tree of Obsidian 1.15 from Flathub, as `flatpak run` started it.
    #[rstest]
    fn the_application_in_a_sandbox_is_found_without_its_launchers() {
        let processes = [
            process(111, 55, "/usr/bin/bwrap", &["bwrap", "--args", "73", "--", "obsidian.sh"]),
            process(145, 1, "/usr/bin/bwrap", &["bwrap", "--args", "76", "--", "xdg-dbus-proxy"]),
            process(151, 111, "/usr/bin/bwrap", &["bwrap", "--args", "73", "--", "obsidian.sh"]),
            process(156, 151, "/app/obsidian", &["/app/obsidian", "--ozone-platform-hint=auto"]),
            process(179, 156, "/app/obsidian", &["/app/obsidian", "--type=zygote"]),
            process(185, 172, "/usr/bin/bwrap", &["bwrap", "--", "zypak-helper", "--type=zygote"]),
            process(195, 151, "/usr/bin/bwrap", &["bwrap", "--", "zypak-helper", "--type=zygote"]),
            process(196, 195, "/app/obsidian", &["/app/obsidian", "--type=zygote"]),
            process(282, 156, "/app/obsidian", &["/app/obsidian", "--type=renderer"]),
        ];
        assert_eq!(find_browsers(&processes), [found(156, "obsidian")]);
    }

    #[rstest]
    #[case::present(
        "[Application]\nname=md.obsidian.Obsidian\n\n[Instance]\ninstance-id=2549460219\n",
        Some(2_549_460_219)
    )]
    #[case::missing("[Application]\nname=md.obsidian.Obsidian\n", None)]
    fn the_instance_is_read_from_the_flatpak_info(
        #[case] info: &str,
        #[case] expected: Option<u64>,
    ) {
        assert_eq!(flatpak_instance(info), expected);
    }

    #[rstest]
    fn a_file_beside_the_own_executable_has_both_views() {
        let exe = std::env::current_exe().unwrap();
        let paths = beside_exe(std::process::id(), "locales").expect("the executable reads");
        assert_eq!(paths.host_view, exe.with_file_name("locales"));
        let mut own_view = PathBuf::from(format!("/proc/{}/root", std::process::id()));
        own_view.push(paths.host_view.strip_prefix("/").unwrap());
        assert_eq!(paths.own_view, own_view);
    }

    #[rstest]
    #[case::plain("1234 (bash) S 1000 1234", Some(1000))]
    #[case::spaces_and_parentheses("99 (Web (Content) 2) S 42 99", Some(42))]
    #[case::cut_short("7 (x)", None)]
    fn the_parent_is_read_after_the_command_name(#[case] stat: &str, #[case] parent: Option<u32>) {
        assert_eq!(parent_of(stat), parent);
    }
}
