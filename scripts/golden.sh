#!/usr/bin/env bash
# The journey gate compares checkpoint frames with goldens in tests/golden/hl/.
# Those frames are rendered from Half-Life data, so they are never tracked
# (tests/golden/ is gitignored). tests/golden-manifest.tsv holds each frame's
# size and sha256, which is all the repository keeps.
#
#   scripts/golden.sh check          fail if a local golden is missing or differs from the manifest
#   scripts/golden.sh bless [DISC]   re-render the goldens from DISC (default dist/hl-psx.cue)
#                                    with psoxide-gate, then rewrite the manifest
#   scripts/golden.sh manifest       rewrite the manifest from the local frames
#
# psoxide-gate is taken from $PSOXIDE_GATE, else from PATH.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
manifest=tests/golden-manifest.tsv
gate=${PSOXIDE_GATE:-psoxide-gate}

write_manifest() {
    {
        printf '# file\tbytes\tsha256 (frames blessed on a disc cooked from a Half-Life install; kept local, see scripts/golden.sh)\n'
        for f in tests/golden/hl/*.png; do
            printf '%s\t%s\t%s\n' "${f#tests/golden/}" \
                "$(wc -c < "$f" | tr -d ' ')" "$(shasum -a 256 "$f" | cut -d' ' -f1)"
        done
    } > "$manifest"
}

case "${1:-check}" in
    check)
        bad=0
        while IFS=$'\t' read -r file bytes sha; do
            case "$file" in '#'* | '') continue ;; esac
            path=tests/golden/$file
            if [[ ! -f $path ]]; then
                echo "missing golden: $path" >&2
                bad=1
            elif [[ $(wc -c < "$path" | tr -d ' ') != "$bytes" ||
                $(shasum -a 256 "$path" | cut -d' ' -f1) != "$sha" ]]; then
                echo "golden differs from the manifest: $path" >&2
                bad=1
            fi
        done < "$manifest"
        if ((bad != 0)); then
            echo "run scripts/golden.sh bless to regenerate them from your own disc" >&2
            exit 1
        fi
        echo "goldens match the manifest"
        ;;
    bless)
        disc=${2:-dist/hl-psx.cue}
        mkdir -p tests/golden/hl
        "$gate" bless tests/journey.toml --no-hw --disc "$disc" --out "${GOLDEN_REPORT_DIR:-golden-report}"
        write_manifest
        ;;
    manifest)
        write_manifest
        ;;
    *)
        echo "usage: $0 check|bless [DISC]|manifest" >&2
        exit 2
        ;;
esac
