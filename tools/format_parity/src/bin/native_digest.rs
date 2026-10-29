//! Prints the parity digest: synthetic blocks by default, or the blocks dumped by `dump_blocks`
//! when a file path is given.
fn main() {
    let digest = match std::env::args_os().nth(1) {
        Some(path) => {
            let bytes = std::fs::read(&path).expect("readable block dump");
            format_parity::parity_digest_of_blocks(&bytes)
        }
        None => format_parity::parity_digest(),
    };
    println!("{digest:016x}");
}
