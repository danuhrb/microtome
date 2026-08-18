# microtome

A fast reader of tiles from whole slide images.

microtome reads tiles from a whole slide image and gives them to Python as
NumPy arrays. The core of the library is Rust code. The library reads many
tiles at the same time. All threads share one file handle. The library
releases the Python global interpreter lock during a read operation.

---

## Installation

```
pip install microtome
```

This command gets a prebuilt wheel. You do not need a compiler. You do not
need OpenSlide. The wheels support Python 3.10 and later versions on Linux
(x86_64, aarch64), macOS (x86_64, arm64), and Windows (x86_64).

---

## Usage

### Open a slide

```python
import microtome as mt

slide = mt.open("CMU-1.svs")

print(slide.dimensions)   # (46000, 32914)
print(slide.mpp)          # 0.499
print(slide.level_count)  # 3
print(slide.levels[0])    # metadata of the base level
```

### Read one tile

Give the level index and the position of the tile in the tile grid.

```python
tile = slide.read_tile(level=0, tx=10, ty=20)
print(tile.shape)         # (256, 256, 3), dtype uint8
```

### Read many tiles

Give a list of tile indices. The tile index counts the tiles of a level from
left to right and from top to bottom. The library reads the tiles in
parallel and returns one array. If you do not give indices, the library
reads all the tiles of the level.

```python
batch = slide.read_batch(level=0, threads=16)
print(batch.shape)        # (23220, 256, 256, 3), dtype uint8
```

### Select a level

The `best_level_for_mpp` function selects a level from the
microns-per-pixel value, not from the magnification label.

```python
level = slide.best_level_for_mpp(2.0)
print(slide.levels[level]["mpp"])
```

---

## Correctness

The test suite compares the output of microtome with the output of OpenSlide
and of libjpeg-turbo. The comparison uses public slides from the OpenSlide
project. More than 99.5 percent of the channel values are equal. No value
differs by more than 1. A difference of 1 comes from rounding in the JPEG
decoder. The comparison script is in the `scripts/` directory.

---

## Speed

One measurement: the base level of `CMU-1.svs` (23220 tiles, JPEG, 4.3 GB
of pixels), warm file cache, 64-core Linux machine.

| Reader          | Threads | Time    |
| --------------- | ------- | ------- |
| OpenSlide 4.0.1 | 1       | 28.6 s  |
| microtome       | 1       | 6.9 s   |
| microtome       | 16      | 0.52 s  |
| microtome       | 64      | 0.28 s  |

OpenSlide uses one thread for each file handle. microtome uses all the
threads that you give it.

---

## Limits

- The library reads Aperio SVS files only. Other formats will come later.
- The library decodes JPEG tiles only. JPEG 2000 support will come later.
- The read functions read stored tiles. They do not read a region at a free
  position. This function will come later.
- The library reads images. It does not write images.
- The library does not read the ICC profile. Color management is your task.

---

## Build from source

Most users do not need this section. Use this procedure only if you change
the code, or if there is no wheel for your system. You need Rust 1.85 or a
later version.

```
pip install maturin
maturin develop --release
```

To run the Rust tests, do this command:

```
cargo test
```

---

## License

This software uses the MIT license. See the `LICENSE` file.
