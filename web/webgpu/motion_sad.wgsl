// Experimental Avelune V1 WebGPU encoder kernel.
// One workgroup evaluates one integer-pixel motion candidate for one 8x8 block.
// Entropy coding and mode decisions remain on CPU/WASM.
struct Params {
  width: u32,
  height: u32,
  block_x: u32,
  block_y: u32,
  radius: i32,
  candidate_width: u32,
};
@group(0) @binding(0) var<storage, read> source: array<u32>;
@group(0) @binding(1) var<storage, read> reference: array<u32>;
@group(0) @binding(2) var<storage, read_write> costs: array<u32>;
@group(0) @binding(3) var<uniform> p: Params;
var<workgroup> partial: array<u32,64>;

fn clamp_i(v:i32, lo:i32, hi:i32)->i32 { return min(max(v,lo),hi); }

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg:vec3u,@builtin(local_invocation_index) li:u32) {
  let ci=wg.x;
  let dx=i32(ci % p.candidate_width) - p.radius;
  let dy=i32(ci / p.candidate_width) - p.radius;
  let x=li & 7u;
  let y=li >> 3u;
  let sx=min(p.block_x+x,p.width-1u);
  let sy=min(p.block_y+y,p.height-1u);
  let rx=u32(clamp_i(i32(sx)+dx,0,i32(p.width)-1));
  let ry=u32(clamp_i(i32(sy)+dy,0,i32(p.height)-1));
  let a=i32(source[sy*p.width+sx] & 255u);
  let b=i32(reference[ry*p.width+rx] & 255u);
  partial[li]=u32(abs(a-b));
  workgroupBarrier();
  var stride=32u;
  loop {
    if li<stride { partial[li]+=partial[li+stride]; }
    workgroupBarrier();
    if stride==1u { break; }
    stride >>= 1u;
  }
  if li==0u { costs[ci]=partial[0]; }
}
