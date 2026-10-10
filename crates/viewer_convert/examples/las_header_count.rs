fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let reader = las::Reader::from_path(&path)?;
        let header = reader.header();
        println!(
            "{}\t{}\t{:?}\t{:?}",
            path,
            header.number_of_points(),
            header.bounds(),
            header.transforms()
        );
    }
    Ok(())
}
