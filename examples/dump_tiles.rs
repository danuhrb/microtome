//! Decode every tile of every level (or one level) and write raw RGB8 to one
//! file per level, plus a metadata file, for comparison against OpenSlide.

use microtome::{file::SlideFile, pool::ThreadPool, schedule, svs::Slide};

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: dump_tiles <slide.svs> <outdir> [threads] [level]";
    let slide_path = args.next().expect(usage);
    let outdir = args.next().expect(usage);

    let file = SlideFile::open(&slide_path).unwrap();
    let slide = Slide::parse(file.bytes()).unwrap();
    let threads = args
        .next()
        .map(|t| t.parse().unwrap())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let only_level: Option<usize> = args.next().map(|l| l.parse().unwrap());
    let pool = ThreadPool::new(threads);
    std::fs::create_dir_all(&outdir).unwrap();

    let mut meta = String::new();
    for (i, level) in slide.levels.iter().enumerate() {
        if only_level.is_some_and(|l| l != i) {
            continue;
        }
        let indices: Vec<usize> = (0..level.tiles.len()).collect();
        let tables = slide.jpeg_tables(level);
        let start = std::time::Instant::now();
        let pixels = schedule::read_tiles(file.bytes(), level, tables, &indices, &pool).unwrap();
        let ms = start.elapsed().as_millis();
        std::fs::write(format!("{outdir}/level{i}.bin"), &pixels).unwrap();
        meta += &format!(
            "{i} {} {} {} {}\n",
            level.width, level.height, level.tile_width, level.tile_height
        );
        println!(
            "level {i}: {}x{}, {} tiles of {}x{}, decoded in {ms} ms on {threads} threads",
            level.width,
            level.height,
            indices.len(),
            level.tile_width,
            level.tile_height
        );
    }
    std::fs::write(format!("{outdir}/meta.txt"), meta).unwrap();
}
