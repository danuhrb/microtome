"""Validate microtome tile output pixel-by-pixel against two references:

- tifffile (libjpeg-turbo): the stored tile data, compared on every level.
- OpenSlide: compared only on levels with exact integer downsamples.
  OpenSlide resamples levels whose downsample is fractional (common in
  Aperio pyramids), so its output is not the stored pixels there.

Usage: validate_openslide.py <slide> <dumpdir>
where <dumpdir> was produced by the dump_tiles example.
"""

import sys

import numpy as np
import openslide
import tifffile

TOLERANCE = 1  # decoder IDCT rounding


def compare_tiles(ours, ref_tile, stats):
    diff = np.abs(ours.astype(np.int16) - ref_tile.astype(np.int16))
    stats[0] += diff.size
    stats[1] += int((diff == 0).sum())
    stats[2] = max(stats[2], int(diff.max()))
    stats[3] += int(diff.sum())


def report(name, stats):
    pixels, exact, max_diff, diff_sum = stats
    print(
        f"  vs {name}: {100.0 * exact / pixels:.3f}% exact, "
        f"max diff {max_diff}, mean diff {diff_sum / pixels:.4f}"
    )
    return max_diff <= TOLERANCE


def main() -> int:
    slide_path, dumpdir = sys.argv[1], sys.argv[2]
    slide = openslide.OpenSlide(slide_path)
    tf = tifffile.TiffFile(slide_path)
    ok = True

    for line in open(f"{dumpdir}/meta.txt"):
        idx, w, h, tw, th = (int(v) for v in line.split())
        across = -(-w // tw)
        down = -(-h // th)
        ours = np.memmap(f"{dumpdir}/level{idx}.bin", dtype=np.uint8, mode="r").reshape(
            across * down, th, tw, 3
        )

        page = next(p for p in tf.pages if p.shape[:2] == (h, w))
        ref = page.asarray(
            out=np.memmap(f"{dumpdir}/ref{idx}.bin", dtype=np.uint8, mode="w+", shape=(h, w, 3))
        )

        os_level = slide.level_dimensions.index((w, h))
        downsample = slide.level_downsamples[os_level]
        os_exact = abs(downsample - round(downsample)) < 1e-9
        ds = round(downsample)

        tf_stats = [0, 0, 0, 0]
        os_stats = [0, 0, 0, 0]
        for ty in range(down):
            for tx in range(across):
                vw = min(tw, w - tx * tw)
                vh = min(th, h - ty * th)
                a = ours[ty * across + tx][:vh, :vw]
                compare_tiles(a, ref[ty * th : ty * th + vh, tx * tw : tx * tw + vw], tf_stats)
                if os_exact:
                    b = np.asarray(
                        slide.read_region((tx * tw * ds, ty * th * ds), os_level, (tw, th)),
                        dtype=np.uint8,
                    )[:vh, :vw, :3]
                    compare_tiles(a, b, os_stats)

        print(f"level {idx}: {across * down} tiles, {w}x{h}")
        ok &= report("tifffile ", tf_stats)
        if os_exact:
            ok &= report("openslide", os_stats)
        else:
            print(f"  vs openslide: skipped (fractional downsample {downsample:.6f})")

    print("PASS" if ok else "FAIL: differences exceed decoder rounding")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
