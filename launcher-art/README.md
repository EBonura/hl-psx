# Launcher art

The pictures the PSoXide demo disc launcher shows for Half-Life: the front cover and
the back-cover shots, listed in `manifest.txt`. They are captures of the game, as
cut by the demo disc project. Nothing here is baked: the demo disc build
bakes these pictures itself (`menu-art ext`), so there are no blobs to go stale.

Use it from a demo-disc checkout by pointing `LAUNCHER_ART` at this directory:

    make launcher LAUNCHER_ART=/path/to/this/repo/launcher-art

Without it the launcher still builds and Half-Life gets the plain cover.

The baker scales every picture to the cover sizes (104x78, 68x51, 52x39) and to
the 64x48 back-cover shots without cropping, and stretches anything that is not
4:3. The front and the back are the same capture.

The `shots` line of the manifest lists the pictures of the shelf's screenshot panel, in order; the disc build cooks them itself.
