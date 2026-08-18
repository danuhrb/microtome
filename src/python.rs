//! Python bindings (enabled with the `python` feature, built by maturin).

use crate::file::SlideFile;
use crate::pool::ThreadPool;
use crate::schedule;
use crate::svs;
use numpy::{IntoPyArray, PyArray3, PyArray4, PyArrayMethods};
use pyo3::exceptions::{PyIOError, PyIndexError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::path::PathBuf;

/// Levels are extracted eagerly because `svs::Slide` borrows the mmap; the
/// Python object stores only owned data plus the mapped file itself.
#[pyclass(frozen)]
pub struct Slide {
    file: SlideFile,
    levels: Vec<svs::Level>,
    jpeg_tables: Vec<Option<Vec<u8>>>,
    #[pyo3(get)]
    mpp: Option<f64>,
    #[pyo3(get)]
    magnification: Option<f64>,
    #[pyo3(get)]
    description: String,
}

fn io_err(e: impl std::fmt::Display) -> PyErr {
    PyIOError::new_err(e.to_string())
}

fn value_err(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}

/// Open a whole slide image.
#[pyfunction]
pub fn open(path: PathBuf) -> PyResult<Slide> {
    let file = SlideFile::open(&path).map_err(io_err)?;
    let parsed = svs::Slide::parse(file.bytes()).map_err(value_err)?;
    let jpeg_tables = parsed
        .levels
        .iter()
        .map(|l| parsed.jpeg_tables(l).map(<[u8]>::to_vec))
        .collect();
    let svs::Slide {
        levels,
        description,
        mpp,
        magnification,
        ..
    } = parsed;
    Ok(Slide {
        file,
        levels,
        jpeg_tables,
        mpp,
        magnification,
        description,
    })
}

impl Slide {
    fn level(&self, index: usize) -> PyResult<&svs::Level> {
        self.levels
            .get(index)
            .ok_or_else(|| PyIndexError::new_err(format!("no level {index}")))
    }

    fn read(&self, level: usize, indices: &[usize], threads: usize) -> PyResult<Vec<u8>> {
        let l = self.level(level)?;
        let tables = self.jpeg_tables[level].as_deref();
        let pool = ThreadPool::new(threads.max(1));
        schedule::read_tiles(self.file.bytes(), l, tables, indices, &pool).map_err(value_err)
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

#[pymethods]
impl Slide {
    /// (width, height) of the base level in pixels.
    #[getter]
    fn dimensions(&self) -> (u64, u64) {
        (self.levels[0].width, self.levels[0].height)
    }

    #[getter]
    fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Metadata for each pyramid level, largest first.
    #[getter]
    fn levels<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.levels
            .iter()
            .map(|l| {
                let d = PyDict::new(py);
                d.set_item("width", l.width)?;
                d.set_item("height", l.height)?;
                d.set_item("tile_width", l.tile_width)?;
                d.set_item("tile_height", l.tile_height)?;
                d.set_item("downsample", l.downsample)?;
                d.set_item("mpp", l.mpp)?;
                d.set_item("tile_count", l.tiles.len())?;
                d.set_item("tiles_across", l.tiles_across())?;
                d.set_item("tiles_down", l.tiles_down())?;
                Ok(d)
            })
            .collect()
    }

    /// Index of the smallest level at least as fine as `mpp`.
    fn best_level_for_mpp(&self, mpp: f64) -> usize {
        let Some(base) = self.mpp else { return 0 };
        let mut best = 0;
        for (i, level) in self.levels.iter().enumerate() {
            if base * level.downsample <= mpp * 1.0001 {
                best = i;
            }
        }
        best
    }

    /// Decode one stored tile of a level as an RGB8 array of shape
    /// (tile_height, tile_width, 3).
    #[pyo3(signature = (level, tx, ty))]
    fn read_tile<'py>(
        &self,
        py: Python<'py>,
        level: usize,
        tx: u64,
        ty: u64,
    ) -> PyResult<Bound<'py, PyArray3<u8>>> {
        let l = self.level(level)?;
        if tx >= l.tiles_across() || ty >= l.tiles_down() {
            return Err(PyIndexError::new_err(format!(
                "tile ({tx}, {ty}) out of range for level {level}"
            )));
        }
        let (tw, th) = (l.tile_width as usize, l.tile_height as usize);
        let index = (ty * l.tiles_across() + tx) as usize;
        let out = py.detach(|| self.read(level, &[index], default_threads()))?;
        out.into_pyarray(py).reshape([th, tw, 3]).map_err(Into::into)
    }

    /// Decode many stored tiles of a level in parallel. Returns an RGB8
    /// array of shape (len(indices), tile_height, tile_width, 3), where
    /// slot j holds tile indices[j] (row-major tile numbering).
    #[pyo3(signature = (level, indices=None, threads=None))]
    fn read_batch<'py>(
        &self,
        py: Python<'py>,
        level: usize,
        indices: Option<Vec<usize>>,
        threads: Option<usize>,
    ) -> PyResult<Bound<'py, PyArray4<u8>>> {
        let l = self.level(level)?;
        let (tw, th) = (l.tile_width as usize, l.tile_height as usize);
        let indices = indices.unwrap_or_else(|| (0..l.tiles.len()).collect());
        let n = indices.len();
        let threads = threads.unwrap_or_else(default_threads);
        let out = py.detach(|| self.read(level, &indices, threads))?;
        out.into_pyarray(py).reshape([n, th, tw, 3]).map_err(Into::into)
    }

    fn __repr__(&self) -> String {
        let (w, h) = self.dimensions();
        let mpp = self
            .mpp
            .map_or("unknown".into(), |m| format!("{m}"));
        format!(
            "<microtome.Slide {w}x{h}, {} levels, mpp={mpp}>",
            self.levels.len()
        )
    }
}

#[pymodule]
fn microtome(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Slide>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
