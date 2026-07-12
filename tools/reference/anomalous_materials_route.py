#!/usr/bin/env python3
"""Build the deterministic Anomalous Materials route through the c1a0b elevator.

The route starts at the retail c1a0 player spawn, crosses into c1a0d, follows
the green personnel stripe, opens the suit case, collects the HEV suit, and
returns along the blue security line. It waits for Barney's retinal scan and
crosses both airlock doors, rides the c1a0a elevator, and follows the lower
laboratory corridor through the real c1a0b changelevel trigger.
It continues through the control-room retinal scan and rotating elevator to
the real c1a0e sample-delivery transition.
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


def c1a0d_hev_to_c1a0a() -> tuple[InputSample, ...]:
    """Return from the HEV cabinet and complete the scripted c1a0a airlock."""
    route = Samples()

    # Drop back into the lower locker aisle and climb its eastern staircase.
    # The wall at x=-3216 requires reaching the upper landing before turning
    # north; cutting that corner leaves the player hull trapped in the pit.
    route.add(20, forward=-127, strafe=-127, actions=ACTION_JUMP)
    route.add(12, strafe=-127)
    route.add(15, forward=-127)
    route.add(3, forward=40)
    route.add(5)
    route.add(8, strafe=127)
    route.add(8, strafe=-40)
    route.add(30)

    # Rejoin the lounge, reopen rr1, and settle on the upper perimeter lane.
    route.add(5, forward=127)
    route.add(8, forward=-40)
    route.add(5)
    route.add(19, strafe=127)
    route.add(8, strafe=-40)
    route.add(5)
    # The PSX fixed-point staircase return settles 19 units farther north than
    # GoldSrc. Two south ticks put both paths inside rr1's fully-open clearance
    # instead of letting the PSX hull graze the translated door slab.
    route.add(2, strafe=-127)
    route.add(60, forward=-127)
    route.add(16, forward=40)
    route.add(5)
    route.add(16, strafe=-127)
    route.add(8, strafe=40)
    route.add(30)

    # Follow the perimeter around the central hall. Staying on the east wall
    # avoids the scripted scientist crossing at (-1884, 424); the small north
    # correction enters the only walkable lane through the lower connector.
    route.add(65, forward=-127)
    route.add(5)
    route.add(65, strafe=-127)
    route.add(5)
    route.add(5, strafe=127)
    route.add(10)
    route.add(90, forward=127)
    route.add(5)

    # Cross the HEV-gated trigger, center both airlock trigger volumes, and
    # preserve the complete Barney retinal-scan sequence before moving again.
    route.add(55, strafe=-127)
    route.add(5)
    route.add(2, strafe=127)
    route.add(10)
    route.add(220)
    route.add(70, forward=-127)
    return tuple(route.values)


def c1a0a_to_c1a0b() -> tuple[InputSample, ...]:
    """Call and ride the elevator, then cross the lower labs into c1a0b."""
    route = Samples()

    # Leave the incoming airlock and approach the external elevator control.
    # The short counter-movements keep both engines off the button's brush.
    route.add(40)
    route.add(20, forward=-127)
    route.add(5, forward=127)
    route.add(5)
    route.add(20, strafe=-127)
    route.add(28, forward=-127)
    route.add(20, strafe=127)
    route.add(12, forward=-127)
    route.add(8)
    route.add(10, strafe=127)
    route.add(8)
    route.add(4, forward=127)
    route.add(6)
    route.add(5, strafe=-127)
    route.add(5)
    route.add(4, look=-127)
    route.add(3, actions=ACTION_USE)
    route.add(4, look=127)
    route.add(100)

    # Enter the arrived car, center inside its upper doorway, and face the
    # interior control. Restoring the view after use leaves the navigation axes
    # unchanged while the player is carried by the real vdeath brush mover.
    route.add(6, strafe=127)
    route.add(2, strafe=-127)
    route.add(15, forward=127)
    route.add(2, forward=-127)
    route.add(10)
    route.add(4, strafe=127)
    route.add(2, strafe=-127)
    route.add(1, forward=-127)
    route.add(1, forward=127)
    route.add(10)
    route.add(8, turn=127)
    route.add(4, turn=127, look=-127)
    route.add(3, actions=ACTION_USE)
    route.add(4, turn=-127, look=127)
    route.add(8, turn=-127)
    route.add(400)

    # Recenter at the lower doorway before walking east. The long pauses in the
    # east hall preserve the retail scientist crossings, whose exact collision
    # timing differs slightly between GoldSrc and PSX fixed-point movement.
    route.add(2, strafe=-127)
    route.add(1, strafe=127)
    route.add(100, forward=-127)
    route.add(40)
    route.add(20, strafe=127)
    route.add(50, forward=-127)
    route.add(30)
    route.add(100, strafe=-127)
    route.add(30)
    route.add(6, forward=127)
    route.add(2, forward=-127)
    route.add(100, strafe=-127)
    route.add(30)

    # Cross the main west corridor. Repeated pushes and waits deterministically
    # resolve the two scripted scientists without depending on their side.
    route.add(2, strafe=127)
    route.add(1, strafe=-127)
    route.add(100, forward=127)
    route.add(30)
    route.add(1, strafe=127)
    route.add(100, forward=127)
    route.add(20)
    route.add(100, forward=127)

    # Explicitly move east before turning south. GoldSrc naturally deflects
    # from the west wall here, but relying on that left PSX in a different lane.
    route.add(10, forward=-127)
    route.add(2, forward=127)
    route.add(8)
    route.add(50, strafe=-127)
    route.add(20)

    # Follow the lower laboratory zig-zag and cross the retail c1a0b trigger.
    route.add(4, strafe=127)
    route.add(2, strafe=-127)
    route.add(100, forward=-127)
    route.add(20)
    route.add(50, forward=-127, strafe=127)
    route.add(20)
    route.add(100, strafe=-127)
    route.add(20)
    route.add(30, forward=-127)
    return tuple(route.values)


def c1a0b_to_c1a0e() -> tuple[InputSample, ...]:
    """Complete the control-room script and descend to sample delivery."""
    route = Samples()

    # Enter the control room through ld2. The east correction clears the open
    # sliding slab before crossing its trigger a second time.
    route.add(40)
    route.add(50, forward=-127)
    route.add(2, forward=127)
    route.add(20, strafe=-127)
    route.add(30)
    route.add(10, forward=-127)
    route.add(2, forward=127)
    route.add(50, strafe=-127)
    route.add(2, strafe=127)

    # Preserve the complete four-scientist briefing and retinal scan, then
    # cross the now-open scanner door into the western elevator corridor.
    route.add(620)
    route.add(100, strafe=-127)
    route.add(40)
    route.add(44, forward=127)
    route.add(2, forward=-127)
    route.add(14, strafe=127)
    route.add(2, strafe=-127)
    route.add(10)

    # Call the rotating elevator from its upper landing. It rises and turns 90
    # degrees before the delayed upper doors open.
    route.add(8, turn=127)
    route.add(3, actions=ACTION_USE)
    route.add(8, turn=-127)
    route.add(130)

    # Align with the narrow round-car doorway, enter it, and settle beside the
    # interior control without relying on collision deflection.
    route.add(18, forward=40)
    route.add(3, forward=-40)
    route.add(5)
    route.add(20, strafe=127)
    route.add(2, strafe=-127)
    route.add(20)
    route.add(8, forward=-40)
    route.add(2, forward=40)
    route.add(5)

    # Face the interior button, start the authentic translating/rotating
    # descent, and restore view input while the pusher carries the player.
    route.add(16, turn=127)
    route.add(3, actions=ACTION_USE)
    route.add(16, turn=-127)
    route.add(200)

    # At the bottom, center the east doorway, turn with the car's final yaw,
    # open edoor_2, and cross into the lower laboratory passage.
    route.add(7, forward=40)
    route.add(2, forward=-40)
    route.add(3)
    route.add(8, turn=-127)
    route.add(3)
    route.add(10, forward=127)
    route.add(25)
    route.add(20, forward=127)
    route.add(2, forward=-127)
    route.add(20)

    # East to the central junction, south around its curved wall, then east
    # through the real c1a0e changelevel volume.
    route.add(50, forward=127)
    route.add(2, forward=-127)
    route.add(5)
    route.add(100, strafe=127)
    route.add(2, strafe=-127)
    route.add(20)
    route.add(30, forward=127)
    return tuple(route.values)


def build_route() -> Route:
    c1a0d = c1a0d_to_hev() + c1a0d_hev_to_c1a0a()
    return Route(
        (
            Segment("c1a0", "c1a0d", rle(c1a0_to_c1a0d()), neutral_tail_ticks=200),
            Segment("c1a0d", "c1a0a", rle(c1a0d), neutral_tail_ticks=200),
            Segment("c1a0a", "c1a0b", rle(c1a0a_to_c1a0b()), neutral_tail_ticks=200),
            Segment("c1a0b", "c1a0e", rle(c1a0b_to_c1a0e()), neutral_tail_ticks=200),
            Segment("c1a0e", "", rle((InputSample(),) * 40), neutral_tail_ticks=200),
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
