#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(git -C "$script_dir" rev-parse --show-toplevel)
cd "$repo_root"

max_source_bytes=$((2 * 1024 * 1024))
source_files=0
source_bytes=0
failed=0

while IFS= read -r -d '' path; do
    source_files=$((source_files + 1))
    size=$(wc -c < "$path")
    source_bytes=$((source_bytes + size))
    lower_path=$(printf '%s' "$path" | tr '[:upper:]' '[:lower:]')

    # Journey tapes are controller input recordings (PXITAPE2: an 8 byte
    # magic, a little-endian u64 sample count, then 6 bytes per sample: two
    # button bytes and four stick bytes), compiled from the text route by
    # tests/chain/hlroute.rs or cut from a recorded playtest. They hold no game
    # or disc data, so they are the one binary kind allowed anywhere under
    # tests/, and only when the layout checks out.
    case "$lower_path" in
        tests/*.pxtape)
            magic=$(head -c 8 "$path")
            count=$(od -An -tu8 -j8 -N8 "$path" | tr -d ' ')
            if [[ "$magic" != PXITAPE2 || -z "$count" ]] || ((size != 16 + 6 * count)); then
                printf 'chain tape is not a well-formed PXITAPE2 input recording: %s\n' "$path" >&2
                failed=1
            fi
            continue
            ;;
    esac

    case "$lower_path" in
        data/*|assets/*|dist/*|captures/*|reference/*)
            printf 'forbidden generated or reference directory: %s\n' "$path" >&2
            failed=1
            ;;
    esac

    case "$lower_path" in
        *.pak|*.vpk|*.gcf|*.wad|*.bsp|*.mdl|*.spr|*.lmp|*.nod|\
        *.hlm|*.hlmdl|*.hltx|*.tex|*.psxm|*.psxc|*.psxw|*.psxa|*.psau|\
        *.bin|*.cue|*.iso|*.img|*.ccd|*.sub|*.chd|*.exe|*.elf|*.map|\
        *.wav|*.mp3|*.ogg|*.flac|*.cdda|*.xa|*.aiff|*.aif|\
        *.tga|*.png|*.jpg|*.jpeg|*.webp|*.gif|*.bmp|\
        *.zip|*.7z|*.rar|*.tar|*.tgz|*.gz|*.bz2|*.xz|\
        *.mp4|*.mov|*.avi|*.mkv)
            printf 'forbidden asset, generated output, or archive: %s\n' "$path" >&2
            failed=1
            ;;
    esac

    if ((size > max_source_bytes)); then
        printf 'tracked file exceeds the 2 MiB source limit: %s (%d bytes)\n' \
            "$path" "$size" >&2
        failed=1
    fi

    encoding=$(file -b --mime-encoding "$path")
    if [[ "$encoding" == "binary" ]]; then
        printf 'tracked binary file is not allowed: %s\n' "$path" >&2
        failed=1
    fi
done < <(git ls-files -z --cached --others --exclude-standard)

if ((failed != 0)); then
    exit 1
fi

printf 'source-only inventory OK: %d files, %d bytes\n' \
    "$source_files" "$source_bytes"
