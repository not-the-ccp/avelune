import fs from 'node:fs';
import crypto from 'node:crypto';

const wasmPath = process.argv[2] ?? 'web/player/avelune-scalar.wasm';
const {instance} = await WebAssembly.instantiate(fs.readFileSync(wasmPath), {});
const ex = instance.exports;
if (ex.avelune_abi_version() !== 0x0001_0000) throw Error(`unexpected ABI ${ex.avelune_abi_version()}`);

function encoderError(handle) {
  const ptr = ex.video_encoder_last_error_ptr(handle), len = ex.video_encoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown encoder error';
}
function decoderError(handle) {
  const ptr = ex.decoder_last_error_ptr(handle), len = ex.decoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown decoder error';
}

const width=32, height=24, frames=9;
const encoder=ex.video_encoder_create(width,height,(30<<16)|1,96,1,4,2<<13);
if (!encoder) throw Error('video_encoder_create failed');
let encoded;
try {
  const frameLen=ex.video_encoder_frame_len(encoder);
  if (frameLen !== width*height*3/2) throw Error(`unexpected frame len ${frameLen}`);
  for (let t=0;t<frames;t++) {
    const ptr=ex.video_encoder_frame_ptr(encoder);
    const bytes=new Uint8Array(ex.memory.buffer,ptr,frameLen);
    const yLen=width*height, cLen=yLen/4;
    for(let y=0;y<height;y++) for(let x=0;x<width;x++) bytes[y*width+x]=(x*5+y*3+t*11)&255;
    bytes.fill(96+t, yLen, yLen+cLen);
    bytes.fill(160-t, yLen+cLen);
    if(ex.video_encoder_push_frame(encoder)!==0) throw Error(encoderError(encoder));
  }
  if(ex.video_encoder_finish(encoder)!==0) throw Error(encoderError(encoder));
  const ptr=ex.video_encoder_output_ptr(encoder), len=ex.video_encoder_output_len(encoder);
  encoded=new Uint8Array(ex.memory.buffer,ptr,len).slice();
  if(!len) throw Error('encoder produced no output');
} finally {
  ex.video_encoder_destroy(encoder);
}

const decoder=ex.decoder_create();
if(!decoder) throw Error('decoder_create failed');
let video=0;
try {
  let offset=0, chunk=1;
  while(offset<encoded.length) {
    const n=Math.min(encoded.length-offset, 1+((chunk*3571)%8191));
    const ptr=ex.input_reserve(decoder,n);
    new Uint8Array(ex.memory.buffer,ptr,n).set(encoded.subarray(offset,offset+n));
    if(ex.decoder_push(decoder,n)!==0) throw Error(decoderError(decoder));
    while(ex.decoder_pop_video(decoder)>0) video++;
    offset+=n; chunk++;
  }
  if(ex.decoder_finish_input(decoder)!==0) throw Error(decoderError(decoder));
  while(ex.decoder_pop_video(decoder)>0) video++;
  if(video!==frames) throw Error(`roundtrip frame count ${video}, expected ${frames}`);
  if(ex.container_stream_count(decoder)!==1 || ex.container_epoch_count(decoder)!==3) throw Error('unexpected encoded index');
} finally { ex.decoder_destroy(decoder); }

console.log(JSON.stringify({wasm:wasmPath,bytes:encoded.length,sha256:crypto.createHash('sha256').update(encoded).digest('hex'),frames:video,epochs:3}));
