# microtome

A fast reader of tiles from whole slide images.

microtome reads small parts of a whole slide image and gives them to Python. The
core of the library is C++ code. The Python layer is thin.

---

## What this library does

A whole slide image is a scan of a glass slide from a microscope. One image has
about 100 000 x 100 000 pixels. One file is 0.5 GB to 3 GB.

You cannot read a file of this size into memory. Thus the file contains a
pyramid of images. Each level of the pyramid has a lower resolution than the
level below it. Each level is divided into small tiles.

microtome reads these tiles. It reads many tiles at the same time.

---



## The problem

Most software reads whole slide images with OpenSlide. OpenSlide is an excellent

library, but it has three limits:

- OpenSlide decodes one tile for each read operation. It uses one thread.
- You cannot safely share one file handle between threads. Thus each worker
process must open its own handle. Each handle keeps its own cache in memory.
- Many slides use JPEG 2000 compression. Free JPEG 2000 decoders are much
slower than JPEG decoders.

Because of these limits, a GPU can use tiles more quickly than the reader can
supply them. The GPU stays idle for much of a training job.

Users usually accept a different procedure. First, they write all the tiles of
all the slides to the disk. Then they train a model from those files. This
procedure needs many hours. It also increases the necessary disk space by 5 to
10 times. If you change the tile size or the magnification, you must do the
procedure again.

microtome removes the need for this procedure.

---



## Functions

- Reads many tiles at the same time with a thread pool.
- Groups tiles that lie next to each other in the file into one read operation.
A request for a hundred tiles becomes a few large reads instead of a hundred
small ones. On object storage, where each read is an HTTP range request, this
removes almost all of the waiting.
- Uses one file handle for all threads. Memory use does not increase with the
number of threads.
- Releases the Python global interpreter lock during a read operation.
- Copies the pixel data one time only. The library writes the pixels directly
into the output array.
- Decodes JPEG tiles and JPEG 2000 tiles.
- Gives the result as a NumPy array or as a PyTorch tensor.
- Selects the resolution from the microns-per-pixel value, not from the
magnification label.

---



## Installation

```
pip install microtome
```

This command gets a prebuilt wheel. The wheel contains the compiled core and
all the necessary libraries. You do not need a compiler. You do not need
CMake. You do not need OpenSlide.

Wheels are available for these systems:


| System  | Architecture    |
| ------- | --------------- |
| Linux   | x86_64, aarch64 |
| macOS   | x86_64, arm64   |
| Windows | x86_64          |


The wheels support Python 3.10 and later versions.

---



## Usage



### Read one tile

```python
import microtome as mt

slide = mt.open("CMU-1.svs")

print(slide.dimensions)        # (46000, 32914)
print(slide.mpp)               # 0.499
print(slide.level_count)       # 3

tile = slide.read_tile(x=12000, y=8000, size=256, mpp=0.5)
print(tile.shape)              # (256, 256, 3)
```



### Read many tiles

Give a list of coordinates to the `read_batch` function. The library reads the
tiles in parallel and returns one array.

```python
coords = [(x, y) for x in range(0, 40000, 256)
                 for y in range(0, 30000, 256)]

batch = slide.read_batch(coords, size=256, mpp=0.5, threads=16)
print(batch.shape)             # (N, 256, 256, 3)
```



### Find the tissue

The `tissue_mask` function reads a low-resolution level and finds the tissue.
It returns the coordinates of the tiles that contain tissue.

```python
coords = slide.tissue_mask(size=256, mpp=0.5)
print(len(coords))             # 9184
```



### Use the library with PyTorch

The `TileDataset` class is compatible with the PyTorch `DataLoader` class. Set
`num_workers` to 0. The library does its own parallel work in C++.

```python
from torch.utils.data import DataLoader
from microtome.torch import TileDataset

dataset = TileDataset("CMU-1.svs", size=256, mpp=0.5)
loader = DataLoader(dataset, batch_size=256, num_workers=0)

for batch in loader:
    features = encoder(batch.cuda())
```

---



## Formats


| Vendor             | Extension | Condition      |
| ------------------ | --------- | -------------- |
| Aperio             | `.svs`    | Supported      |
| Generic tiled TIFF | `.tif`    | Supported      |
| Hamamatsu          | `.ndpi`   | In development |
| Leica              | `.scn`    | Planned        |
| Philips            | `.tiff`   | Planned        |
| Ventana            | `.bif`    | Planned        |
| MIRAX              | `.mrxs`   | Not planned    |


The first version reads Aperio SVS files only. This format is the most common
format in public data sets. Other formats will come later.

---



## Speed

Speed data will be added here after the first release. Each measurement will
include these conditions:

- The name and the size of the slide file.
- The compression of the tiles: JPEG or JPEG 2000.
- The tile size and the microns-per-pixel value.
- The number of threads.
- The type of the disk: local NVMe, network file system, or object storage.
- The version of each library in the comparison.

The comparison will include OpenSlide and cuCIM. The test script is in the
`bench/` directory. All test slides are public. Thus you can do the same
measurements.

---



## Correctness

microtome compares its output with the output of OpenSlide. The test suite
reads every tile of each test slide with both libraries. The pixels must be
equal.

The test data comes from the OpenSlide project. See
`https://openslide.cs.cmu.edu/download/openslide-testdata/`.

If you find a difference between the two libraries, make a report in the issue
tracker. Include the name of the file and the coordinates of the tile.

---



## Limits

- The library reads images. It does not write images.
- The library reads brightfield images only. It does not read fluorescence
images.
- The library does not read the ICC profile. Color management is your task.
- The library does not do stain normalization.
- The Python API needs Python 3.10 or a later version.

---



## Build from source

Most users do not need this section. The `pip install` command gets a prebuilt
wheel. Use this procedure only if you change the code, or if there is no wheel
for your system.

### What you need

- A C++ compiler with support for C++23. The core uses
`std::move_only_function`, so the standard library must define
`__cpp_lib_move_only_function`. GCC 12 or later and MSVC 2022 17.2 or later
provide it. libc++ provides it only from LLVM 22, and only with
`-fexperimental-library`, so on macOS use GCC or a recent LLVM.
- CMake, version 3.20 or later.
- Python 3.10 or later, with the development headers.
- These libraries: libjpeg-turbo, OpenJPEG, libtiff, and zlib.

The build system downloads pybind11 automatically. You do not need to install
it.

### Install the libraries

On Debian or Ubuntu, do this command:

```
sudo apt install build-essential cmake python3-dev \
    libjpeg-turbo8-dev libopenjp2-7-dev libtiff-dev zlib1g-dev
```

On macOS, do this command:

```
brew install cmake jpeg-turbo openjpeg libtiff
```

On Windows, use vcpkg:

```
vcpkg install libjpeg-turbo openjpeg tiff zlib
```



### Build the Python module

```
git clone https://github.com/danuhrb/microtome
cd microtome
pip install -e .
```

The `pip` command starts CMake and compiles the core. The first build needs
about two minutes. Later builds are faster, because CMake keeps a cache.

To build with debug symbols, set this variable first:

```
CMAKE_BUILD_TYPE=RelWithDebInfo pip install -e .
```



### Build the C++ library only

Use this procedure if you do not need the Python module.

```
cmake -B build -DCMAKE_BUILD_TYPE=Release -DMICROTOME_PYTHON=OFF
cmake --build build -j
ctest --test-dir build
```



### Run the tests

1. Download the test slides. Use the `scripts/get_testdata.py` script.
2. Run `pytest tests/`.

---



## How to help

The project accepts changes to the code. Before you write a large quantity of
code, make an issue and describe your plan.

These tasks are open:

- Make the JPEG 2000 decoder faster.
- Add support for the Hamamatsu NDPI format.
- Add a reader for object storage with HTTP range requests.
- Add more test slides to the test suite.

---



## Related software


| Name       | Description                                   |
| ---------- | --------------------------------------------- |
| OpenSlide  | The C library that reads most vendor formats. |
| cuCIM      | A reader from NVIDIA that uses a GPU.         |
| tiffslide  | A reader in Python for TIFF-based slides.     |
| libvips    | A general library for large images.           |
| TIAToolbox | A tool set for computational pathology.       |


microtome does not replace OpenSlide. OpenSlide reads more formats. Use
microtome when you must read many tiles quickly from a supported format.

---



## License

This software uses the MIT license. See the `LICENSE` file.