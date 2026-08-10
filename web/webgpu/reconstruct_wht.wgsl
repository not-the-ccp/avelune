// Experimental ALV1 WebGPU decoder kernel.
// One workgroup reconstructs one complete 8x8 residual block. Coefficients are
// already entropy-decoded/dequantized by CPU/WASM; prediction samples are supplied
// in raster order. Arithmetic mirrors the normative V1 integer inverse WHT.
struct Params { block_count: u32, };
@group(0) @binding(0) var<storage, read> coeffs: array<i32>;
@group(0) @binding(1) var<storage, read> prediction: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;
@group(0) @binding(3) var<uniform> p: Params;
var<workgroup> a: array<i32,64>;

fn round_div_64(v:i32)->i32 {
  if v >= 0 { return (v + 32) / 64; }
  return -((-v + 32) / 64);
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg:vec3u,@builtin(local_invocation_index) li:u32) {
  let bi=wg.x;
  if bi>=p.block_count { return; }
  let base=bi*64u;
  a[li]=coeffs[base+li];
  workgroupBarrier();

  let x=li & 7u;
  let y=li >> 3u;
  var span=1u;
  loop {
    let in_pair=x % (span*2u);
    if in_pair<span {
      let j=y*8u + (x-in_pair) + in_pair;
      let k=j+span;
      let av=a[j]; let bv=a[k];
      a[j]=av+bv; a[k]=av-bv;
    }
    workgroupBarrier();
    if span==4u { break; }
    span*=2u;
  }

  span=1u;
  loop {
    let in_pair=y % (span*2u);
    if in_pair<span {
      let j=((y-in_pair)+in_pair)*8u+x;
      let k=j+span*8u;
      let av=a[j]; let bv=a[k];
      a[j]=av+bv; a[k]=av-bv;
    }
    workgroupBarrier();
    if span==4u { break; }
    span*=2u;
  }

  let r=round_div_64(a[li]);
  let v=clamp(i32(prediction[base+li])+r,0,255);
  output[base+li]=u32(v);
}
