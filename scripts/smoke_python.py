"""Smoke test for the Python bindings against a real slide."""

import sys
import time

import numpy as np
import microtome as mt

slide = mt.open(sys.argv[1] if len(sys.argv) > 1 else "data/CMU-1.svs")
print(repr(slide))
print("dimensions :", slide.dimensions)
print("mpp        :", slide.mpp)
print("mag        :", slide.magnification)
print("level_count:", slide.level_count)
for i, lv in enumerate(slide.levels):
    print(f"level {i}: {lv}")

level = slide.best_level_for_mpp(2.0)
print("best level for mpp=2.0:", level)

tile = slide.read_tile(0, 1, 1)
print("read_tile  :", tile.shape, tile.dtype, "mean", tile.mean().round(2))

start = time.perf_counter()
batch = slide.read_batch(0)
ms = (time.perf_counter() - start) * 1000
n = batch.shape[0]
print(f"read_batch : {batch.shape} {batch.dtype} in {ms:.0f} ms ({n / ms * 1000:.0f} tiles/s)")

# Tile (1,1) must occupy slot across+1 of the full batch
lv = slide.levels[0]
assert np.array_equal(batch[lv["tiles_across"] + 1], tile)
assert batch.dtype == np.uint8
print("OK")
