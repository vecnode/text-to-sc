#!/bin/sh
# install.sh - installs dsh-supercollider into the DeepSeek Harness web profile
# on macOS and Linux, straight from this repository.
#
# The vncode pack installs this package with that repository's scripts/install.sh.
# This is the standalone path: it registers THIS checkout's plugin/ folder with
# the harness, so a change made here is live after a restart with no re-install.
#
# The Windows half is scripts/install.ps1 and does the same work with the same
# flags. This half never needs PowerShell, and never will: Node.js with npm/npx
# and nothing else.
#
# Usage:
#   ./scripts/install.sh [--plugin <dir>] [--profile <name>] [--dsh-home <dir>]
#                        [--force] [--uninstall] [--help]

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
plugin_path="$repo_root/plugin"
profile_name="${DSH_PROFILE:-web}"
dsh_home="${DSH_HOME:-$HOME/.dsh}"
force=0
uninstall=0

usage() {
    cat <<'EOF'
Usage: ./scripts/install.sh [flags]

  --plugin <dir>     the package folder to install (default: this repo's plugin/)
  --profile <name>   harness profile (default: web, else $DSH_PROFILE)
  --dsh-home <dir>   harness home (default: $DSH_HOME, else ~/.dsh)
  --force            re-add even when the version is unchanged
  --uninstall        remove the package instead of adding it
  --help             this message
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --plugin) plugin_path="$2"; shift 2 ;;
        --profile) profile_name="$2"; shift 2 ;;
        --dsh-home) dsh_home="$2"; shift 2 ;;
        --force) force=1; shift ;;
        --uninstall) uninstall=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "unknown flag: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ ! -f "$plugin_path/package.json" ]; then
    echo "No package.json in $plugin_path - pass --plugin with the package folder." >&2
    exit 1
fi

package_name=$(node -e 'process.stdout.write(require(process.argv[1]).name)' "$plugin_path/package.json")
package_version=$(node -e 'process.stdout.write(require(process.argv[1]).version)' "$plugin_path/package.json")
profile_dir="$dsh_home/profiles/$profile_name"

if ! command -v npx >/dev/null 2>&1; then
    echo 'npx was not found - install Node.js 22 or newer.' >&2
    exit 1
fi

echo ''
echo "== dsh-supercollider installer ($package_name@$package_version)"
echo "  package   $plugin_path"
echo "  DSH home  $dsh_home"
echo "  profile   $profile_name"
[ -d "$profile_dir" ] || echo '  (the profile directory does not exist yet; the harness initializes it on add)'

# The harness pins its pnpm layout in the profile and a `dsh plugin add` works
# out which one that is; the environment is left as the caller had it.
DSH_HOME="$dsh_home" export DSH_HOME

if [ "$uninstall" -eq 1 ]; then
    echo ''
    echo '== Removing from the profile'
    npx --yes @deepseek-ai/dsh plugin --profile "$profile_name" remove "$package_name"
else
    echo ''
    echo '== Adding to the profile'
    npx --yes @deepseek-ai/dsh plugin --profile "$profile_name" add "$plugin_path"

    # The skills also ship as plain files, which is what lets an editor-less
    # profile and `npx skills add` see the same documents.
    echo ''
    echo '== Copying the bundled skills into the harness skills root'
    skills_root="$dsh_home/skills"
    copied=''
    if [ -d "$plugin_path/skills" ]; then
        for skill_dir in "$plugin_path"/skills/*/; do
            [ -d "$skill_dir" ] || continue
            [ -f "$skill_dir/SKILL.md" ] || continue
            name=$(basename "$skill_dir")
            mkdir -p "$skills_root/$name"
            cp -R "$skill_dir"/. "$skills_root/$name"/
            copied="${copied}${copied:+, }$name"
        done
    fi
    if [ -n "$copied" ]; then
        echo "  copied: $copied -> $skills_root"
    else
        echo '  no skill folders found (nothing copied)'
    fi
fi

cat <<'EOF'

== Done

  Restart the DeepSeek Harness app (Ctrl+C the `npx @deepseek-ai/dsh web`
  process and start it again), then hard-refresh the browser.

  Requirements: SuperCollider 3.13 or newer, installed the normal way. Nothing
  else: the plugin is plain JavaScript with no dependencies, and it finds
  SuperCollider itself.

  Then ask the agent for something in SuperCollider, or open the SuperCollider
  console tab in the right bar and type a line.
EOF
