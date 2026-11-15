use wgpu::util::DeviceExt;
const SRC: &str = r#"
@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read_write> o: array<f32>;
@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) g: vec3<u32>) {
  let i = g.x; if i >= arrayLength(&o) / 2u { return; }
  let origin = a[3u*i]; let r = a[3u*i+1u]; let side = a[3u*i+2u];
  o[2u*i] = origin + (r + 0.5) * side;
  o[2u*i+1u] = fma(r + 0.5, side, origin);
}"#;
fn main() {
    let n = 100_000usize;
    let mut s = 0x12345678u64;
    let mut rnd = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s };
    let mut input = Vec::with_capacity(3*n);
    for _ in 0..n {
        let origin = (rnd() % 2_000_000) as f32 * 0.37 - 370_000.0 ;
        let rel = (rnd() % 65536) as f32;
        let side = 0.0788f32 * ((rnd() % 1000) as f32 + 1.0) * 1e-3;
        input.extend([origin, rel, side]);
    }
    for (name, fb) in [("real", false), ("fallback", true)] {
        let inst = wgpu::Instance::default();
        let Ok(ad) = pollster::block_on(inst.request_adapter(&wgpu::RequestAdapterOptions{force_fallback_adapter: fb, power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default()})) else { println!("{name}: no adapter"); continue; };
        let info = ad.get_info();
        let (dev, q) = pollster::block_on(ad.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let sm = dev.create_shader_module(wgpu::ShaderModuleDescriptor{label:None, source: wgpu::ShaderSource::Wgsl(SRC.into())});
        let p = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:None, layout:None, module:&sm, entry_point:Some("main"), compilation_options:Default::default(), cache:None});
        let ib = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor{label:None, contents: bytemuck::cast_slice(&input), usage: wgpu::BufferUsages::STORAGE});
        let size = (2*n*4) as u64;
        let ob = dev.create_buffer(&wgpu::BufferDescriptor{label:None,size,usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_SRC,mapped_at_creation:false});
        let rb = dev.create_buffer(&wgpu::BufferDescriptor{label:None,size,usage:wgpu::BufferUsages::MAP_READ|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor{label:None, layout:&p.get_bind_group_layout(0), entries:&[wgpu::BindGroupEntry{binding:0,resource:ib.as_entire_binding()},wgpu::BindGroupEntry{binding:1,resource:ob.as_entire_binding()}]});
        let mut e = dev.create_command_encoder(&Default::default());
        { let mut c = e.begin_compute_pass(&Default::default()); c.set_pipeline(&p); c.set_bind_group(0,&bg,&[]); c.dispatch_workgroups((n as u32+63)/64,1,1); }
        e.copy_buffer_to_buffer(&ob,0,&rb,0,size);
        q.submit([e.finish()]);
        rb.slice(..).map_async(wgpu::MapMode::Read, |_|{});
        dev.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let out: Vec<f32> = bytemuck::cast_slice(&rb.slice(..).get_mapped_range().unwrap()).to_vec();
        let (mut plain_eq_unfused, mut plain_eq_fused, mut fma_eq_fused, mut fused_ne_unfused) = (0,0,0,0);
        for i in 0..n {
            let (o, r, sd) = (input[3*i], input[3*i+1], input[3*i+2]);
            let unf = o + (r + 0.5) * sd; let fu = (r+0.5).mul_add(sd, o);
            if unf.to_bits()!=fu.to_bits() { fused_ne_unfused += 1; }
            if out[2*i].to_bits()==unf.to_bits() { plain_eq_unfused+=1; }
            if out[2*i].to_bits()==fu.to_bits() { plain_eq_fused+=1; }
            if out[2*i+1].to_bits()==fu.to_bits() { fma_eq_fused+=1; }
        }
        println!("{name} {:?} {}: n={n} fused!=unfused {fused_ne_unfused} | plain==unfused {plain_eq_unfused} plain==fused {plain_eq_fused} | fma()==mul_add {fma_eq_fused}", info.backend, info.name);
    }
}
