#!/usr/bin/env bash
# Placeholder stand-ins for the cooked data/ files game/build.rs reads, so CI
# can type-check the game without Half-Life assets. Every value is a dummy:
# never pack or run a build made from them. Refuses to touch a real data/.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(git -C "$script_dir" rev-parse --show-toplevel)
data="$repo_root/data"
if [[ -e "$data" ]]; then
    printf '%s exists; refusing to overwrite cooked assets\n' "$data" >&2
    exit 1
fi
mkdir -p "$data/voices"

# Every skill.cfg cvar the game names (shared/hl-format/src/skill.rs is the one
# list build.rs reads), at all three skill levels.
grep -o '"sk_[A-Za-z0-9_]*"' "$repo_root/shared/hl-format/src/skill.rs" | tr -d '"' | sort -u |
    while read -r cvar; do
        printf '%s1 "1"\n%s2 "1"\n%s3 "1"\n' "$cvar" "$cvar" "$cvar"
    done > "$data/skill.cfg"

# build.rs insists on at least one NPC use reply.
printf '0|0|use:barney:start\n' > "$data/voices/manifest.txt"

# build.rs looks up a hitbox mask for every (actor type, hit group) pair in
# hitgroup::RULES; one placeholder mask for every plausible pair covers them.
mkdir -p "$data/modelpack"
for type in $(seq 0 127); do
    for group in $(seq 0 15); do
        printf '%s|%s|1\n' "$type" "$group"
    done
done > "$data/modelpack/hitgroups.txt"
