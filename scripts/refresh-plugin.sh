#!/usr/bin/env bash
# Refreshes the copy of the fwd plugin that Claude Code runs, then checks that the copy matches the
# checkout it came from. Claude Code copies a plugin into its plugin cache when it installs it
# (~/.claude/plugins/cache/fwdcentaur/fwd/<version>), and sessions run that copy, not the checkout,
# so a change to the plugin (its hook or its skills) pulled into the checkout reaches no session
# until the copy is refreshed (integrations/claude-code/README.md, "Updating").
#
#   scripts/refresh-plugin.sh           runs claude plugin update fwd@fwdcentaur, then the check
#   scripts/refresh-plugin.sh --check   the check alone
#
# The update makes a new copy only when the plugin's version differs from the copy's. The plugin's
# manifest (integrations/claude-code/.claude-plugin/plugin.json) names no version, so Claude Code
# takes the commit the checkout is on as the version: once the checkout is on another commit, the
# update copies the plugin's folder as it stands, untracked files included, into a new directory of
# the cache named after the commit's first 12 characters, and marks the previous copy to be deleted
# 14 days later. While the manifest named 0.1.0, the update said the plugin was at the latest
# version and left the copy as it was first installed (task dcAEY3Wz).
#
# The check compares the copy Claude Code records for fwd@fwdcentaur at user scope (claude plugin
# list --json) with the plugin's folder in the checkout the fwdcentaur marketplace was added from
# (claude plugin marketplace list --json), file by file: each file's content, and whether it is
# executable. It leaves out the two markers Claude Code writes into a copy, .in_use (the sessions
# that run it) and .orphaned_at (a copy an update replaced). It exits 0 when the two match, 1 when
# a file differs, naming each, and 2 when it cannot check. A change to the plugin that is not
# committed moves no version, so the update leaves the copy as it was and the check names the
# changed files: commit the change and run this again.
#
# A running session goes on with the copy it loaded until /reload-plugins, and a new session loads
# the new copy. The script finds Claude Code's configuration where Claude Code does, so
# CLAUDE_CONFIG_DIR points both at another configuration, such as a scratch one for a test.
set -euo pipefail

marketplace=fwdcentaur
plugin=fwd
id="$plugin@$marketplace"

case "${1:-}" in
    "") update=yes ;;
    --check) update=no ;;
    *)
        echo "usage: scripts/refresh-plugin.sh [--check]" >&2
        exit 2
        ;;
esac
for tool in claude jq; do
    if ! command -v "$tool" >/dev/null; then
        echo "refresh-plugin.sh: $tool is not on the PATH" >&2
        exit 2
    fi
done
cannot_check() {
    echo "refresh-plugin.sh: $1, so the copy was not checked" >&2
    exit 2
}

# Prints the version and the folder of the copy Claude Code records for the plugin at user scope,
# separated by a tab, or nothing when the plugin is not installed there.
installed() {
    claude plugin list --json |
        jq -r --arg id "$id" '.[] | select(.id == $id and .scope == "user") | [.version, .installPath] | @tsv'
}

record="$(installed)" || cannot_check "claude plugin list --json failed"
if [ -z "$record" ]; then
    cannot_check "Claude Code has no install of $id at user scope (integrations/claude-code/README.md, \"Installing\", says how to install it)"
fi
IFS=$'\t' read -r old_version copy <<<"$record"
version="$old_version"
if [ "$update" = yes ]; then
    claude plugin update "$id" || cannot_check "claude plugin update $id failed"
    record="$(installed)" || cannot_check "claude plugin list --json failed"
    [ -n "$record" ] || cannot_check "Claude Code has no install of $id at user scope after the update"
    IFS=$'\t' read -r version copy <<<"$record"
fi

# The plugin's folder in the checkout the marketplace was added from, where the marketplace's own
# manifest puts it (./integrations/claude-code).
checkout="$(claude plugin marketplace list --json |
    jq -r --arg name "$marketplace" '.[] | select(.name == $name) | .installLocation')" ||
    cannot_check "claude plugin marketplace list --json failed"
if [ -z "$checkout" ] || [ ! -f "$checkout/.claude-plugin/marketplace.json" ]; then
    cannot_check "Claude Code has no marketplace named $marketplace with a .claude-plugin/marketplace.json"
fi
plugin_path="$(jq -r --arg name "$plugin" '.plugins[] | select(.name == $name) | .source | strings' \
    "$checkout/.claude-plugin/marketplace.json")" ||
    cannot_check "$checkout/.claude-plugin/marketplace.json could not be read"
plugin_path="${plugin_path#./}"
if [ -z "$plugin_path" ]; then
    cannot_check "$checkout/.claude-plugin/marketplace.json names no folder in the checkout for $plugin"
fi
folder="$checkout/$plugin_path"
for dir in "$folder" "$copy"; do
    [ -d "$dir" ] || cannot_check "$dir is not a folder"
done

# Prints every file under a folder as a path relative to it, leaving out Claude Code's markers.
files() {
    (cd "$1" && find . \( -path ./.in_use -o -path ./.orphaned_at \) -prune -o ! -type d -print) |
        sed 's|^\./||'
}
executable() {
    if [ -x "$1" ]; then echo yes; else echo no; fi
}
differences=()
while IFS= read -r path; do
    if [ ! -e "$copy/$path" ]; then
        differences+=("$path is in the checkout and not in the copy")
    elif [ ! -e "$folder/$path" ]; then
        differences+=("$path is in the copy and not in the checkout")
    elif ! cmp -s "$folder/$path" "$copy/$path"; then
        differences+=("$path differs")
    elif [ "$(executable "$folder/$path")" != "$(executable "$copy/$path")" ]; then
        differences+=("$path is executable in one and not in the other")
    fi
done < <(LC_ALL=C sort -u <(files "$folder") <(files "$copy"))

if [ ${#differences[@]} -eq 0 ]; then
    echo "The copy Claude Code runs matches the checkout: $copy (version $version) and $folder."
    if [ "$version" != "$old_version" ]; then
        echo "The update replaced the copy of version $old_version with this one: a running session goes on with the copy it loaded until /reload-plugins, and a new session loads this one."
    fi
    exit 0
fi
echo "The copy Claude Code runs differs from the checkout: $copy (version $version) against $folder."
printf '  %s\n' "${differences[@]}"
pinned="$(jq -r '.version // empty' "$folder/.claude-plugin/plugin.json" 2>/dev/null || true)"
if [ -n "$pinned" ]; then
    echo "The plugin's manifest names version $pinned, so Claude Code makes a new copy only when that version changes: take the version out of $folder/.claude-plugin/plugin.json (integrations/claude-code/README.md, \"Updating\", says why)."
elif [ -n "$(git -C "$checkout" --no-optional-locks status --porcelain --untracked-files=all -- "$plugin_path" 2>/dev/null)" ]; then
    echo "The plugin's folder in the checkout has changes that are not committed (git status lists them). Claude Code takes the checkout's commit as the plugin's version, so a change that is not committed moves no version and the update leaves the copy as it was: commit the change, then run scripts/refresh-plugin.sh again."
elif [ "$update" = no ]; then
    echo "scripts/refresh-plugin.sh, without --check, updates the copy."
else
    echo "The update found the copy made from the commit the checkout is on, so the copy, or a file git ignores, changed after Claude Code made it: claude plugin uninstall $id, then claude plugin install $id, makes a new copy."
fi
exit 1
