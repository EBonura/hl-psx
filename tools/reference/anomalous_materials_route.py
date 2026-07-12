#!/usr/bin/env python3
"""Build the deterministic Anomalous Materials route through HEV acquisition.

The route starts at the retail c1a0 player spawn, crosses into c1a0d, follows
the green personnel stripe, opens the suit case, and collects the HEV suit.
It deliberately ends at that milestone; the return trip to the blue security
line and c1a0a is appended only after it has its own GoldSrc proof.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from hlinput import (
    ACTION_JUMP,
    ACTION_USE,
    InputSample,
    Route,
    Segment,
    rle,
    write_route,
)


class Samples:
    def __init__(self) -> None:
        self.values: list[InputSample] = []

    def add(self, ticks: int, **values: int) -> None:
        self.values.extend([InputSample(**values)] * ticks)


def c1a0_to_c1a0d() -> tuple[InputSample, ...]:
    """Security desk, upper corridor, and the c1a0d transition."""
    route = Samples()
    route.add(200)  # scripted entry doors and guard settle
    route.add(35, forward=127)
    route.add(1, strafe=127)
    route.add(3, actions=ACTION_USE)  # desk buzzer
    route.add(180)  # preserve the guard/scientist exchange

    # North through the security room, west along the upper corridor, then
    # around the central bend to the western transition volume.
    route.add(9, strafe=127)
    route.add(2, forward=127, strafe=-30)
    route.add(7, forward=127)
    route.add(20, strafe=127)
    route.add(4, forward=127, strafe=-60)
    route.add(70, forward=127)
    route.add(28, strafe=-127)
    route.add(17, forward=-127, strafe=-80)
    route.add(40, forward=127)
    return tuple(route.values)


def c1a0d_to_hev() -> tuple[InputSample, ...]:
    """Carried c1a0d spawn through the green line and HEV pickup."""
    route = Samples()
    route.add(20)

    # Center the down-ramp exit, then follow the green stripe to the lounge.
    # Low-speed corrections avoid pinning the player hull on doorway jambs.
    route.add(30, forward=127)
    route.add(21, forward=-40)
    route.add(10)
    route.add(50, strafe=127)
    route.add(60, forward=127)
    route.add(16, forward=-40)
    route.add(10)
    route.add(44, strafe=40)
    route.add(10)
    route.add(80, forward=127)  # opens and crosses rr1

    # Locker-bank upper aisle and the HEV control pocket.
    route.add(30, strafe=-127)
    route.add(30, forward=127)
    route.add(25, strafe=127)
    route.add(4, forward=-127)
    route.add(4, forward=80, strafe=127)
    route.add(20)
    route.add(3, strafe=-127)
    route.add(2, strafe=127)
    route.add(6)

    # Aim at the small case control, use it, and restore yaw/pitch exactly.
    route.add(5, turn=-127, look=-127)
    route.add(3, actions=ACTION_USE)
    route.add(5, turn=127, look=127)
    route.add(50)  # case door reaches its permanent raised position

    # Return to the ledge, drop to the lower aisle, and align with the first
    # eight-unit stair. The final west+north running jump enters the open case.
    route.add(8, strafe=-127)
    route.add(8, forward=127)
    route.add(20, strafe=-127, actions=ACTION_JUMP)
    route.add(25, forward=127)
    route.add(18, forward=-40)
    route.add(10)
    route.add(5, strafe=127)
    route.add(4, forward=127, strafe=-127)
    route.add(5, forward=127)
    route.add(20, forward=127, strafe=127, actions=ACTION_JUMP)
    route.add(80)
    return tuple(route.values)


def build_route() -> Route:
    return Route(
        (
            Segment("c1a0", "c1a0d", rle(c1a0_to_c1a0d()), neutral_tail_ticks=200),
            Segment("c1a0d", "", rle(c1a0d_to_hev()), neutral_tail_ticks=200),
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="output HLINPUT1 tape")
    args = parser.parse_args()
    route = build_route()
    write_route(args.output, route)
    print(
        f"wrote {args.output}: "
        f"{', '.join(segment.map for segment in route.segments)} / "
        f"{sum(segment.total_ticks for segment in route.segments)} ticks"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
