#!/usr/bin/env python3
"""Build the clips manifest from the model roster entries (stdin).

Input lines:  "type|model|specs"  where specs = comma tokens:
  - "12:4"            index-labelled clip (unnamed slot)
  - "pondering:2"     name-labelled clip (named slot)
  - "pondering2=pondering"  alias -> a named slot
  - "idle1=@0"        alias -> a slot number directly
Output lines: "type|name|slot"  (data/models/clips.txt, read by the map cook
to resolve scripted_sequence m_iszPlay/m_iszIdle names into clip slots).
Pure metadata derived from our own roster; contains no asset data.
"""
import sys

out = []
for line in sys.stdin:
    line = line.strip()
    if not line or line.startswith('#'):
        continue
    ty, _model, specs = line.split('|', 2)
    slot = 0
    named = {}
    aliases = []
    for tok in specs.split(','):
        tok = tok.strip()
        if not tok:
            continue
        if '=' in tok:
            a, b = tok.split('=', 1)
            aliases.append((a.strip().lower(), b.strip().lower()))
            continue
        label = tok.split(':', 1)[0].strip()
        if not label.lstrip('-').isdigit():
            named[label.lower()] = slot
        slot += 1
    for name, s in named.items():
        out.append(f"{ty}|{name}|{s}")
    for a, b in aliases:
        if b.startswith('@') and b[1:].isdigit():
            out.append(f"{ty}|{a}|{int(b[1:])}")
        elif b in named:
            out.append(f"{ty}|{a}|{named[b]}")
        else:
            print(f"warn: alias {a}={b} target unknown (type {ty})", file=sys.stderr)

print('\n'.join(out))
