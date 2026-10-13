//! Inline layout diagnostic, excluding owned buffers and allocator overhead.
use std::{
    io::Cursor,
    mem::{align_of, size_of},
};

use viewer_format::FormatReader;
use viewer_render_wgpu::{InstanceRaw, RenderScene};
use viewer_wasm::{LodDataset, ViewerController};

fn report<T>(name: &str) {
    println!(
        "{name}: inline={} align={} box_handle={} box_payload={} box_sum={}",
        size_of::<T>(),
        align_of::<T>(),
        size_of::<Box<T>>(),
        size_of::<T>(),
        size_of::<Box<T>>() + size_of::<T>(),
    );
}

fn main() {
    println!(
        "arch={} pointer_bits={}",
        std::env::consts::ARCH,
        usize::BITS
    );
    println!("box_sum includes the handle and boxed object, not its buffers or allocator overhead");
    report::<ViewerController>("ViewerController");
    report::<LodDataset>("LodDataset");
    report::<FormatReader<Cursor<Vec<u8>>>>("FormatReader<Cursor<Vec<u8>>>");
    report::<RenderScene>("RenderScene");
    report::<InstanceRaw>("InstanceRaw");
}
