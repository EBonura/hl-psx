#!/bin/bash
# usage: mapnav.sh PLAY_ORDER_INDEX [start_tick]  -> prints --pad-pulses string that enters that map from the title menu
P=$1; T=${2:-800}
START=(0 6 11 18 23 28 37 40 50 56 60 64 74 77 84 85 88 94 95)
DOWN=0x0040; CROSS=0x4000
out=""; t=$T
press(){ out="$out${out:+,}$1@$t+4"; t=$((t+16)); }
if [ $P -ge 96 ]; then
  press $DOWN; press $CROSS
  for ((i=0;i<P-96;i++)); do press $DOWN; done; press $CROSS
else
  c=0; for i in "${!START[@]}"; do [ ${START[$i]} -le $P ] && c=$i; done
  r=$((P-START[c]))
  for i in 1 2 3 4; do press $DOWN; done; press $CROSS
  for ((i=0;i<c;i++)); do press $DOWN; done; press $CROSS
  for ((i=0;i<r;i++)); do press $DOWN; done; press $CROSS
fi
echo "$out"
