# Contributing to microtome

Thank you for your interest in contributing to microtome! There are many ways
to contribute, and we appreciate all of them.

If you have questions, open a discussion or an issue on the repository.

As a reminder, all contributors are expected to be respectful and
constructive in issues, reviews, and discussions.

- [Feature Requests](#feature-requests)
- [Bug Reports](#bug-reports)
- [Building and Testing](#building-and-testing)
- [Pull Requests](#pull-requests)
- [Writing Documentation](#writing-documentation)
- [Finding Something to Work On](#finding-something-to-work-on)
- [Helpful Links and Information](#helpful-links-and-information)

## Feature Requests

Before you file a feature request, check the issue tracker and the
[Finding Something to Work On](#finding-something-to-work-on) section below:
the feature may already be planned or in progress. When you
file one, describe the use case, not only the mechanism — "I need to read
Hamamatsu slides from S3" is more useful than "add an S3 client".

Large features should start as an issue and a short design discussion before
any large amount of code is written.

## Bug Reports

While bugs are unfortunate, they're a reality in software. We can't fix what
we don't know about, so please report liberally.

The most useful bug report for a slide-reading library names the file. If
you can, report:

- The slide file, or a link to it if it is public (the
  [OpenSlide test data](https://openslide.cs.cmu.edu/download/openslide-testdata/)
  is a shared reference corpus).
- If the file is private, the output of the `print_tags` example
  (`cargo run --example print_tags <file>`), which contains the TIFF
  structure and no pixel data.
- What you expected and what happened, with the exact error message.
- For pixel-correctness bugs: the level and tile coordinates, and the output
  of `scripts/validate_openslide.py` if you can run it.

Crashes and panics on malformed files are always bugs, even if the file is
corrupt. The parser must fail with an error, never a panic.

## Building and Testing

The core library is plain Rust:

```sh
cargo build
cargo test
```

The Python bindings are behind the `python` feature and are built with
[maturin](https://github.com/PyO3/maturin):

```sh
pip install maturin
maturin develop --release
python scripts/smoke_python.py <slide.svs>
```

`cargo test` runs against small fixture files committed in the repository
and needs no downloads. The full validation compares output against
OpenSlide and libjpeg-turbo on real slides:

```sh
# one-time setup
python3 -m venv data/venv
data/venv/bin/pip install openslide-bin openslide-python numpy tifffile imagecodecs
curl -L -o data/CMU-1.svs https://openslide.cs.cmu.edu/download/openslide-testdata/Aperio/CMU-1.svs

# run
cargo run --release --example dump_tiles data/CMU-1.svs data/dump
data/venv/bin/python scripts/validate_openslide.py data/CMU-1.svs data/dump
```

A pull request that touches the parser or the decoder should pass this
validation, not only the unit tests.

## Pull Requests

Pull requests are the primary mechanism we use to change microtome. GitHub
has some [great documentation][pr-docs] on using the Pull Request feature.

[pr-docs]: https://docs.github.com/en/pull-requests

Some guidelines:

- Keep PRs focused. One feature or one fix per PR reviews faster than a
  grab bag.
- Add tests with the code they test. Parser changes need a fixture or a
  synthetic file built in the test; decoder changes need a pixel-level
  assertion.
- Run `cargo fmt` and `cargo clippy` before pushing; CI checks both test
  configurations (`cargo test` and `--features python`).
- No `unsafe` without a `// Safety:` comment explaining the invariant. The
  existing unsafe block in `schedule.rs` is the pattern to follow.
- Write commit messages that say why, not only what.

CI must be green before merge. If CI fails for a reason unrelated to your
change, say so in a comment.

## Writing Documentation

Documentation improvements are very welcome and are a good first
contribution. The README is written in ASD-STE100 Simplified Technical
English: short sentences, active voice, one instruction per sentence. Match
that register when editing it. Rustdoc comments on public items follow
normal prose rules.

## Finding Something to Work On

These are the known gaps and ideas, roughly grouped. Items marked **(good
first issue)** are self-contained. Open an issue before starting anything
large.

### File formats

- **JPEG 2000 decoding** — the single most impactful gap. Many real Aperio
  archives use compression 33003/33005. Plan: OpenJPEG via FFI behind the
  existing `Decoder` trait, or a pure-Rust decoder if one matures.
- **Generic tiled TIFF** — the TIFF layer already parses these; a
  vendor-neutral path around the Aperio-specific checks in `svs.rs` would
  open every tiled-TIFF slide. **(good first issue)**
- **Trestle** — parses today but needs tile-overlap handling to render
  seamlessly (see `data/CMU-1.tif` for a test file).
- **Hamamatsu NDPI** — TIFF-like with vendor quirks (64-bit offsets hidden
  in private tags).
- **Philips TIFF, Leica SCN** — planned, further out.
- **DICOM WSI** — increasingly common in hospital PACS; large but valuable.

### API

- **Coordinate-based region reads** — `read_region(x, y, size, mpp)` that
  crops and stitches across tile boundaries and resamples to the requested
  mpp. This is the README-promised API and the largest missing piece.
- **`tissue_mask`** — find tissue-bearing tiles from a low-resolution level.
- **PyTorch `TileDataset`** — a `python/` shim over `read_batch`, plus
  DLPack export so tensors avoid a NumPy round-trip.
- **Type stubs** — a `.pyi` file so IDEs autocomplete the Python API.
  **(good first issue)**
- **Label/macro image access** — the stripped IFDs are parsed but not
  exposed. **(good first issue)**

### Performance

- **abi3 wheels** — build one wheel per platform instead of one per Python
  version (`pyo3/abi3-py310`). Shrinks the CI matrix and speeds releases.
  **(good first issue)**
- **Reuse the thread pool across calls** — `read_batch` builds a pool per
  call; a pool cached on the `Slide` object saves spawn cost for many small
  batches.
- **`madvise` hints** — `WILLNEED` on coalesced ranges before decode would
  help cold-cache reads; the `Read` ranges from `coalesce` are exactly the
  input needed.
- **Object storage reader** — HTTP range requests driven by the same
  coalesced ranges; the design anticipates this but nothing is implemented.
- **GPU decode** — nvImageCodec/nvJPEG2000 behind the `Decoder` trait as an
  optional feature. Most useful for JPEG 2000 archives.
- **Benchmarks** — a `bench/` suite with cold/warm cache, JPEG 2000, and
  cuCIM comparison, with slide names, tile sizes, storage types, and library
  versions recorded.

### Robustness

- **Fuzzing** — `cargo-fuzz` targets for `Tiff::parse` and `Slide::parse`.
  The parsers are bounds-checked but have never been fuzzed. High value for
  hospital-archive files.
- **Miri** — run the test suite under Miri in CI to check the unsafe block
  in `schedule.rs`.
- **Validation breadth** — extend `validate_openslide.py` to sweep the whole
  OpenSlide Aperio corpus instead of two slides.
- **ICC profile** — expose the raw profile bytes so callers can do color
  management. **(good first issue)**

## Helpful Links and Information

- [OpenSlide test data](https://openslide.cs.cmu.edu/download/openslide-testdata/) — public slides
- [TIFF 6.0 specification](https://download.osgeo.org/libtiff/doc/TIFF6.pdf)
- [BigTIFF design](https://www.awaresystems.be/imaging/tiff/bigtiff.html)
- [OpenSlide format documentation](https://openslide.org/formats/) — vendor
  format details, including Aperio and Trestle quirks
- [PyO3 user guide](https://pyo3.rs/) and
  [maturin documentation](https://www.maturin.rs/)
