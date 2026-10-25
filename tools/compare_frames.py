#!/usr/bin/env python3
"""Compare a native frame (s4play S4_FRAME_DUMP .ppm) with a screenshot of the original under Wine.
   tools/compare_frames.py native.ppm wine_screenshot.png [diff.png]
Both are reduced to RGB565 first (the game's pixel format), and the 800x600 game area is located in the
screenshot automatically. Prints the number of differing pixels and their bounding box."""
import sys
from PIL import Image, ImageChops

def q(im):
    r, g, b = im.convert('RGB').split()
    return Image.merge('RGB', (r.point(lambda v: v >> 3), g.point(lambda v: v >> 2), b.point(lambda v: v >> 3)))

native, shot = Image.open(sys.argv[1]).convert('RGB'), Image.open(sys.argv[2]).convert('RGB')
qn, (w, h) = q(native), native.size
best = None
for oy in range(0, 48):
    for ox in range(0, 8):
        d = ImageChops.difference(q(shot.crop((ox, oy, ox + w, oy + h))), qn)
        s = sum(i % 256 * v for i, v in enumerate(d.histogram()))
        if best is None or s < best[0]:
            best = (s, ox, oy)
_, ox, oy = best
d = ImageChops.difference(q(shot.crop((ox, oy, ox + w, oy + h))), qn).convert('L').point(lambda v: 255 if v else 0)
n = sum(d.histogram()[255:])
print(f"game area at ({ox},{oy}); {n} of {w * h} pixels differ; bbox {d.getbbox()}")
if len(sys.argv) > 3:
    d.save(sys.argv[3])
