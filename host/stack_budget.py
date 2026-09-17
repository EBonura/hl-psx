#!/usr/bin/env python3
"""Prove the scratchpad projection stack fits its guarded budget.

`game/src/scratchpad.rs` runs the model projection chain on the 1 KiB CPU
scratchpad, growing down from byte 1024 towards a canary that ends at
`PROJECTION_GUARD_END`. The runtime guard notices an overrun only afterwards
and then moves the path back to main RAM for the rest of the session, so an
inlining change (a profile-guided build grew this chain from 568 to 824
bytes) costs speed without any sign of it. This check walks the linked
image's static call graph from the chain's entry and fails the build instead.

    python3 stack_budget.py game.exe link.map scratchpad.rs

Needs mipsel-none-elf-objdump on PATH, or another one named in OBJDUMP.
"""
import bisect
import os
import re
import subprocess
import sys

ENTRY = "hl_psx::project_hmd7_model_stack_entry"
HEADER = 0x800
LOAD_ADDR = 0x80010000
SCRATCHPAD_BYTES = 1024
# Never return, so whatever they push is never popped over live scratchpad.
DIVERGING = ("core::panicking::", "__rustc::rust_begin_unwind")


def text_symbols(map_path):
    """(address, size, name) for every function, from an LLD link map."""
    symbols, text_end = [], None
    for line in open(map_path, errors="replace"):
        fields = line.split(maxsplit=4)
        if len(fields) != 5:
            continue
        try:
            address, size = int(fields[0], 16), int(fields[2], 16)
        except ValueError:
            continue
        name = fields[4].strip()
        if name.startswith("__text_end"):
            text_end = address
        if size and not name.startswith(("/", ".", "<internal>")) and ":(" not in name and " = " not in name:
            symbols.append((address, size, re.sub(r"::h[0-9a-f]{16}$", "", name)))
    if text_end is None:
        raise SystemExit("stack budget: no __text_end in %s" % map_path)
    return sorted(s for s in symbols if LOAD_ADDR <= s[0] < text_end)


def disassemble(exe_path):
    image = exe_path + ".text.tmp"
    with open(exe_path, "rb") as source, open(image, "wb") as out:
        out.write(source.read()[HEADER:])
    try:
        listing = subprocess.run(
            [os.environ.get("OBJDUMP", "mipsel-none-elf-objdump"), "-D", "-b", "binary", "-m", "mips:3000",
             "-EL", "--adjust-vma=%#x" % LOAD_ADDR, image],
            check=True, capture_output=True, text=True).stdout
    finally:
        os.remove(image)
    code = {}
    for line in listing.splitlines():
        match = re.match(r"([0-9a-f]{8}):\s+[0-9a-f]{8}\s+(\S+)\s*(.*)", line)
        if match:
            code[int(match.group(1), 16)] = (match.group(2), match.group(3))
    return code


def main():
    exe_path, map_path, scratchpad_rs = sys.argv[1:4]
    guard_end = re.search(r"const PROJECTION_GUARD_END: usize = (\d+);", open(scratchpad_rs).read())
    if guard_end is None:
        raise SystemExit("stack budget: PROJECTION_GUARD_END not found in %s" % scratchpad_rs)
    budget = SCRATCHPAD_BYTES - int(guard_end.group(1))

    symbols = text_symbols(map_path)
    starts = [s[0] for s in symbols]
    code = disassemble(exe_path)

    def function_at(address):
        index = bisect.bisect_right(starts, address) - 1
        if index >= 0 and address < symbols[index][0] + symbols[index][1]:
            return index
        return None

    def depth(index, path):
        address, size, name = symbols[index]
        if name.startswith(DIVERGING):
            return 0, []
        if index in path:
            raise SystemExit("stack budget: %s recurses, so its depth has no bound" % name)
        frame, deepest = 0, (0, [])
        for pc in range(address, address + size, 4):
            op, args = code.get(pc, ("", ""))
            grow = re.match(r"sp,sp,-(\d+)", args) if op == "addiu" else None
            if grow:
                frame = max(frame, int(grow.group(1)))
            elif op == "jalr":
                raise SystemExit("stack budget: %s calls through a register at %08x" % (name, pc))
            elif op in ("jal", "j"):
                # A `j` into another function is a tail call; one into the
                # hazard trampolines (data) comes straight back.
                callee = function_at(int(args, 16))
                if callee is not None and callee != index:
                    below = depth(callee, path | {index})
                    if below[0] > deepest[0]:
                        deepest = below
        return frame + deepest[0], ["%s(%d)" % (name.split("::")[-1], frame)] + deepest[1]

    entries = [i for i, s in enumerate(symbols) if s[2] == ENTRY]
    if len(entries) != 1:
        raise SystemExit("stack budget: expected one %s, found %d" % (ENTRY, len(entries)))
    total, chain = depth(entries[0], frozenset())
    print("scratchpad projection stack: %d of %d bytes via %s" % (total, budget, " > ".join(chain)))
    if total > budget:
        raise SystemExit("stack budget: the chain overruns its guard; shrink a frame or lower "
                         "PROJECTION_GUARD_END (never below the save record at byte 140)")


if __name__ == "__main__":
    main()
