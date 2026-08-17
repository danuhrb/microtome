//! Print every IFD entry of a TIFF file, decoding common tags.

use microtome::{file::SlideFile, tiff::Tiff};

fn main() {
    let path = std::env::args().nth(1).expect("usage: print_tags <file>");
    let file = SlideFile::open(&path).unwrap();
    let tiff = Tiff::parse(file.bytes()).unwrap();
    for (i, ifd) in tiff.ifds.iter().enumerate() {
        println!("IFD {i} at {}:", ifd.offset);
        for e in &ifd.entries {
            let value = match tiff.uints(e) {
                Ok(v) if v.len() <= 8 => format!("{v:?}"),
                Ok(v) => format!("[{} values, first {:?}]", v.len(), &v[..4]),
                Err(_) => tiff
                    .ascii(e)
                    .map(|s| format!("{:?}", &s[..s.len().min(80)]))
                    .unwrap_or_else(|_| format!("({} bytes)", e.count)),
            };
            println!("  tag {:5} type {:2} count {:6} = {value}", e.tag, e.ty, e.count);
        }
    }
}
