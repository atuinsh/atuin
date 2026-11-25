# Reads Obsidian, an Electron application from Flathub, in its Flatpak sandbox: as `flatpak run`
# starts it, with ACCESSIBILITY_ENABLED=1 in the environment of the program that starts it, as
# a session has it after `atuin setup`, and with the variable passed in with --env. Needs
# `flatpak --user install flathub md.obsidian.Obsidian`.
#
# The sandbox hides org.a11y.Bus from Obsidian, so Obsidian joins the accessibility bus only
# with the variable. Without the variable, capture must not read anything of Obsidian.
#
# Apps installed with `flatpak --user` live in the account's own home, which the session
# replaces with a private one.
export FLATPAK_USER_DIR=$HARNESS_USER_HOME/.local/share/flatpak
if ! flatpak --user info md.obsidian.Obsidian > /dev/null 2>&1; then
    echo "skip  Obsidian is not installed with flatpak --user"
    return 0
fi
keep_a11y
sleep 1
for variant in default session enabled; do
    capture_start 'apps = ["obsidian"]' 'poll_interval = "0s"' || return
    sleep 1
    mark "Obsidian, $variant"
    if [ $variant = enabled ]; then
        flatpak run --env=ACCESSIBILITY_ENABLED=1 md.obsidian.Obsidian --password-store=basic > "$W/obsidian-$variant.out" 2>&1 &
    elif [ $variant = session ]; then
        ACCESSIBILITY_ENABLED=1 flatpak run md.obsidian.Obsidian --password-store=basic > "$W/obsidian-$variant.out" 2>&1 &
    else
        flatpak run md.obsidian.Obsidian --password-store=basic > "$W/obsidian-$variant.out" 2>&1 &
    fi
    sleep 15
    can_focus && focus obsidian
    sleep 5
    echo "doctor, $variant:"
    "$ATUIN" desktop-capture doctor 2> /dev/null | grep -iE '^(app |obsidian|electron)'
    capture_stop
    flatpak kill md.obsidian.Obsidian 2> /dev/null
    sleep 3
done
records | grep -v unchanged
echo
records | sed -n '/^== Obsidian, default/,/^== Obsidian, session/p' | grep -qE '^segment [0-9a-f-]+: .*snapshot, [0-9]{2,} nodes' \
    && echo "FAIL  Obsidian is read without the variable" || echo "ok    Obsidian isn't read without the variable"
records | sed -n '/^== Obsidian, session/,/^== Obsidian, enabled/p' | grep -qE '^segment [0-9a-f-]+: .*snapshot, [0-9]{2,} nodes' \
    && echo "ok    Obsidian is read with the variable in the session" || echo "FAIL  Obsidian is not read with the variable in the session"
records | sed -n '/^== Obsidian, enabled/,$p' | grep -qE '^segment [0-9a-f-]+: .*snapshot, [0-9]{2,} nodes' \
    && echo "ok    Obsidian is read with ACCESSIBILITY_ENABLED=1" || echo "FAIL  Obsidian is not read with ACCESSIBILITY_ENABLED=1"
