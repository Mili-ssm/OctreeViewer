use std::{env, fs::File};
use viewer_format::{FormatReader, FormatReaderLimits};

fn main() {
    let path = env::args().nth(1).expect("expected primary file path");
    let eager = FormatReader::new(File::open(&path).unwrap()).unwrap();
    let lazy =
        FormatReader::open_lazy(File::open(path).unwrap(), FormatReaderLimits::default()).unwrap();
    let anchor = lazy
        .page_source()
        .unwrap()
        .root_unloaded()
        .first()
        .expect("fixture must have a child page");
    println!("{} {}", eager.reference_spacing().unwrap(), anchor.get());
}
