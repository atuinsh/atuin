# Desktop capture: what each environment provides

What each environment that capture runs in provides, and the runs that checked
setup there. [Setup](setup.md) describes the design that follows from these
facts, in [Setup on Linux](setup.md#setup-on-linux) and [Setup on
macOS](setup.md#setup-on-macos).

## Linux

### What the environment has to provide

The table below collects what capture depended on in each environment. It comes
from probes of GTK 3, GTK 4, Qt 5, Qt 6, Firefox, Chromium, and VS Code in
headless sessions, under sway and under X11, in containers of each
distribution. The variables were cleared, and then each one was set alone. It
also comes from real logins in virtual machines: Fedora 45 GNOME, Ubuntu 26.04
GNOME with its snaps, Ubuntu 24.04 GNOME, Debian 13 GNOME and XFCE, Kubuntu
26.04 Plasma, openSUSE Tumbleweed with the openSUSEway sway session, and Arch
with sway, Hyprland, and i3 started from a console, with Bash and with Zsh. The
toolkits' rules come from their source: qtbase's `dbusconnection.cpp`,
Chromium's `atk_util_auralinux.cc`, Gecko's `accessible/atk/Platform.cpp`, and
at-spi2-core's `at-spi-bus-launcher.c`.

| Application | What makes it join | With capture connected, nothing else set |
|---|---|---|
| GTK 3 | always, unless `NO_AT_BRIDGE=1` | readable |
| GTK 4 | always, unless `GTK_A11Y=none` | readable |
| Qt 6 | the flag, followed live, or `QT_LINUX_ACCESSIBILITY_ALWAYS_ON` | readable on 2.61+; never on 2.60 and earlier |
| Qt 5.15, Wayland | never: the native Wayland plugin lacks a bridge | never; `QT_QPA_PLATFORM=xcb` makes it readable |
| Qt 5.15, X11 | the same as Qt 6, but registration with the registry is a race | sometimes |
| Firefox | `GNOME_ACCESSIBILITY` (`0` turns it off), else the flag when a window takes the focus, else `toolkit-accessibility` when the bus cannot be asked | readable on 2.61+ once focused; never on 2.60 and earlier |
| Chromium, Electron | at start: the first of `ACCESSIBILITY_ENABLED`, `GNOME_ACCESSIBILITY`, `QT_ACCESSIBILITY` that is `1` or `0`, else the flag, else `toolkit-accessibility` when the bus cannot be asked | readable if started after capture, or with a variable |
| Flatpak Chromium and Electron | only the variables: the sandbox cannot ask the bus | only with a variable |
| Snap Firefox and Chromium | the same as unconfined: `desktop-legacy` is connected automatically and grants the accessibility bus, and session variables reach the snap | the same as unconfined |

### Qt 5 on X11

Qt 5.15 on X11 reads the address of the bus from the `AT_SPI_BUS` property of
the root window. The bus launcher sets that property whenever the launcher
starts with a `DISPLAY`. When Qt finds the property, Qt's connection object
turns itself on and announces that in its constructor. The bridge doesn't
listen for the announcement yet at that moment, so the bridge registers with
the registry only when the replies arrive in a lucky order. In tests,
sqlitebrowser and MuseScore 2 joined in some rounds and not in others under the
same conditions, with every connection to the bus in place. Nothing that setup
can set changes this, so Qt 5 applications stay best effort.

### What each session provides by default

| Session | at-spi2-core | Flag | Variables | Session file that reaches applications |
|---|---|---|---|---|
| Fedora 45, GNOME | 2.62.1 | follows listeners | none | `environment.d`, `~/.bash_profile` |
| Arch, sway, Hyprland, i3 from a console | 2.62.1 | follows listeners | none | `~/.bash_profile`, or `.zprofile` with Zsh |
| Ubuntu 26.04, GNOME | 2.60.4 | mirrors `toolkit-accessibility` | `QT_ACCESSIBILITY=1` | `environment.d`, `~/.profile` |
| Kubuntu 26.04, Plasma | 2.60.4 | mirrors `toolkit-accessibility` | `QT_ACCESSIBILITY=1` | `environment.d`, `~/.profile`, `plasma-workspace/env` |
| Debian 13, GNOME (GDM 48, Wayland) | 2.56.2 | mirrors `toolkit-accessibility` | `QT_ACCESSIBILITY=1`, `GTK_MODULES=gail:atk-bridge` | `environment.d`, `~/.profile`, because the skeleton doesn't have a `~/.bash_profile` |
| Debian 13, XFCE from LightDM (X11) | 2.56.2 | mirrors `toolkit-accessibility` | `QT_ACCESSIBILITY=1` | `~/.xsessionrc` only. `~/.profile` isn't read |
| Ubuntu 24.04, GNOME (GDM 46, Wayland) | 2.52.0 | mirrors `toolkit-accessibility` | `QT_ACCESSIBILITY=1`, `GTK_MODULES=gail:atk-bridge` | `environment.d`, `~/.profile` |
| openSUSE Tumbleweed, openSUSEway sway from greetd | 2.62.1 | follows listeners | `NO_AT_BRIDGE=1` from `/etc/sway/env` | `environment.d`, which `sway-run.sh` reads after `/etc/sway/env`, and `~/.profile`, which greetd sources before `/etc/sway/env` |

Other versions of at-spi2-core as of November 2026: Fedora 44 2.60.7, Fedora 43
2.58.9, AlmaLinux 10 2.56.1, Debian 12 2.46.0, Ubuntu 22.04 2.44.0, and
AlmaLinux 9 2.40.3. In openSUSE's stock sway session, GTK 3 applications never
join, because of `NO_AT_BRIDGE=1`. Setup's file in `environment.d` sets
`NO_AT_BRIDGE=0`, which overrides it. In a login after setup, sway, the service
manager, and their children had `NO_AT_BRIDGE=0`, and gedit and waybar joined.
After capture was turned off, the session had `NO_AT_BRIDGE=1` again.
`/etc/sway/env` comes with `sway-branding-openSUSE` only, and the upstream
branding doesn't set `NO_AT_BRIDGE`. openSUSE's GSettings backend is keyfile.
Setup wrote `toolkit-accessibility` to `~/.config/glib-2.0/settings/keyfile`,
and turning capture off put back `false`.

Kubuntu 26.04's X11 Plasma session comes with the `plasma-session-x11` package.
It read setup's files as the Wayland session did. After setup set
`toolkit-accessibility`, Kate and the Firefox snap were readable in Kubuntu's
Plasma, on Wayland and on X11.

### The files that each session reads

A variable reaches an application only through the files that the application's
session reads at login. The display managers and session wrappers of Debian 13,
Ubuntu 26.04, Fedora 45, and Arch read these files of the user. The table comes
from the scripts that each package ships. Logins in virtual machines confirmed
these rows:

- GNOME from GDM: Fedora 45 with Bash and Zsh, Ubuntu 26.04 with Bash and fish,
  Debian 13, and Ubuntu 24.04. With fish, `environment.d` was present too, so
  the effect of fish's own file wasn't isolated.
- SDDM and KDE Plasma: Kubuntu 26.04 with Bash, on X11 and on Wayland.
- LightDM on Debian: XFCE on Debian 13, which read `~/.xsessionrc`.
- The console login: Arch with Bash, for sway, Hyprland, and
  `startx /usr/bin/i3`, and with Zsh for sway. With Zsh, `.zprofile` is the
  only file that runs before `exec sway`. A bare `startx` wasn't tested.

The other rows weren't tested in a login.

| Session | Files of the user that it reads |
|---|---|
| GNOME from GDM | `environment.d`, and the login shell's file, because gnome-session starts itself again through the login shell when `/etc/shells` lists that shell |
| Another Wayland session from GDM, such as KDE Plasma, niri, sway, or Hyprland | no file of the user: GDM starts the session's command directly. Plasma imports `environment.d` from the service manager at its start, and so do sessions that run their compositor as a systemd unit, such as `niri-session` and uwsm. sway and Hyprland started directly don't read either |
| KDE Plasma | `environment.d`, the files of the SDDM row, and `plasma-workspace/env` |
| SDDM, X11 and Wayland | Bash: the first of `~/.bash_profile`, `~/.bash_login`, and `~/.profile`. Zsh: `.zprofile`. fish: `~/.profile` and fish's own configuration. Other shells: `~/.profile`. X11 only: also `~/.xprofile` |
| LightDM on Arch | the same as SDDM on X11 |
| LightDM on Ubuntu (`lightdm-session`) | `~/.profile`, `~/.xprofile`, `~/.xsessionrc` |
| LightDM on Debian (`/etc/X11/Xsession`) | `~/.xsessionrc` only |
| LightDM on Fedora (`/etc/X11/xinit/Xsession`) | `~/.profile`, and then the login shell's file |
| An X11 session from GDM, such as XFCE or i3 (`/etc/gdm/Xsession`) | `~/.profile`, `~/.xprofile`, and on Debian `~/.xsessionrc` |
| A console login that starts sway, Hyprland, or `startx` | the login shell's file only, up to the line that `exec`s the compositor |

XFCE and other X11 desktops never read `environment.d`. The applications that
they start inherit the environment that the display manager's wrapper built.
Bash reads only the first of `~/.bash_profile`, `~/.bash_login`, and
`~/.profile`, and Fedora and Arch create `~/.bash_profile` for every new user.
fish as a login shell reads `config.fish` and `~/.config/fish/conf.d/`, and
Nushell 0.115 reads `env.nu`, `config.nu`, and `login.nu`. Neither reads
`~/.profile`. Every display manager above runs `pam_env`, which reads
`/etc/environment`, and systemd links `/etc/environment` into `environment.d`.
That one file would reach every session, but only root can write it.

## macOS

### Fresh accounts

The steps of [Setup on macOS](setup.md#setup-on-macos) were checked on
2026-11-21 in fresh virtual machines of macOS 13.7.4, 14.8.7, and 15.7.7, with
an ad-hoc signed helper, as `cargo install` builds it. On each version, setup
in an admin account that never had the grant opened the Accessibility pane, the
grant asked for the account's password, setup saw the grant at once, and
capture stored the text of TextEdit and Safari. In a standard account, setup
created the certificate and signed the helper, and capture worked after an
administrator's grant. Capture also went on after a logout and a login of
either account. On 13.7.4, `status` said "waiting for a desktop session to
start" during a logout, and capture ran again within four seconds of the next
login.

Capture after a restart was tested on 13.7.4, 14.8.7, and 15.7.7 with automatic
login. launchd's `sh.atuin.daemon` agent, which `atuin setup` installs, started
the daemon, and the daemon ran the signed copy of the helper. TextEdit and
Safari were captured without any command.

When the account didn't have a login keychain yet, as before its first login at
the console, the search list held only the signing keychain after the
certificate was created. The first login added `login.keychain-db` to the
search list as the default, and kept the signing keychain on the list. On
15.7.7, a rebuilt helper was signed again by a restart of the daemon and by
`atuin setup`, and it kept the grant without a prompt.

### The lock screen and other users

On 15.7.7, loginwindow became the active application about a millisecond before
the watcher's look that found the session locked. An earlier build followed
loginwindow as the active window and stored the lock screen as
`loginwindow "Login"`, with the date, the time, and the full names of the Mac's
users. After the fix, three lock cycles didn't store anything of the lock
screen, `status` said "paused while the screen is locked" during each lock, the
segments ended at the lock, and a TextEdit window that opened while the screen
was locked was stored by the sweep after the unlock.

A switch to another user ends the helper at once, because this user leaves the
console, and `status` says "waiting for a desktop session to start" until the
switch back. On 15.7.7, reads were intact after each of these: a switch to a
second user who then logged out from the Apple menu, twice, and a switch to a
user in the setup assistant whose session ended with
`sudo launchctl bootout gui/<uid>`, twice.

`tccutil reset Accessibility` left a running helper capturing for at least 10
minutes, because `AXIsProcessTrusted` stayed true in that process. The switch
in System Settings stopped capture within 2 seconds.

### The permission prompts

On fresh accounts on macOS 13, 14, and 15, the running helper picked up the
grant without a restart. The switch in the Accessibility pane asked an
administrator account "Enter your password to allow this", with its name filled
in, and a standard account "Enter an administrator's name and password to allow
this". The test account on the Mac with macOS 27 is a standard account.

The system's alert, "would like to control this computer", opened behind System
Settings on 14 and 15, and partly above it on 13. The alert stayed open after
the grant on 13, 14, and 15. On 13.7.4, the alert closed at once when the
helper asked it to quit. The alert's window is titled "Accessibility Access" on
13 to 15 and "Device Control and Data Access" on 27. Capture leaving out its
process, `universalAccessAuthWarn`, was verified on 14.8.7 and 15.7.7 with
`apps = "all"`. Setup starts the daemon before it asks for the grant, so the
daemon's first helper lacks the permission, and the daemon tries a helper again
only a minute later. Setup therefore asks the daemon to start a helper once
setup sees the grant. On 14.8.7, capture then ran within 5 seconds of the
grant, where it had taken up to 61 seconds.

### Applications

VS Code exposed its editor text within 3 seconds of `AXManualAccessibility` on
macOS 13 and 15. On 14, the text appeared only after the window was activated
again. A VS Code that had just launched missed the switch on its first
activation on 13, 14, and 15. With `"editor.accessibilitySupport": "auto"` and
the second request of `quirks::WAKE_AGAIN`, such a VS Code had its editor text
in the segment after 6.5 seconds, in two launches on each of 13, 14, and 15.
Without the second request, the editor text was missing from every launch.
`AXEnhancedUserInterface` returns success on 13 and "not implemented" on 14 and
15, and it didn't change any content on any version.

### A managed Mac

A managed Mac is untested, because no MDM was available. According to Apple's
documentation, a privacy profile that grants Accessibility installs only
through an MDM enrollment that the user approved, and it names the application
by its code requirement. An organisation can therefore approve only a helper
with a signature that the organisation knows, such as a Developer ID signature,
and never a certificate that setup made on the user's machine. The profile's
Accessibility grant is deprecated from macOS 26.2 and removed in 27.0, in
favour of a declarative setting that the user can change in System Settings.
