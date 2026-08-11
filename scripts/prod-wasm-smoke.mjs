import fs from 'node:fs';
const wasmPath=process.argv[2]||'web/player/avelune-prod-scalar.wasm';
const mediaPath=process.argv[3]||'web/player/demo.avl';
const {instance}=await WebAssembly.instantiate(fs.readFileSync(wasmPath),{}), ex=instance.exports;
const h=ex.decoder_create(); if(!h) throw Error('create failed');
const bytes=fs.readFileSync(mediaPath); let off=0, chunks=0;
while(off<bytes.length){ const n=Math.min(bytes.length-off, 1+((chunks*37)%4093)); const p=ex.input_reserve(h,n); new Uint8Array(ex.memory.buffer,p,n).set(bytes.subarray(off,off+n)); if(ex.decoder_push(h,n)!==0){const ep=ex.decoder_last_error_ptr(h),en=ex.decoder_last_error_len(h);throw Error(new TextDecoder().decode(new Uint8Array(ex.memory.buffer,ep,en)));} off+=n; chunks++; }
const frontLen=ex.container_front_len(h),streams=ex.container_stream_count(h),epochs=ex.container_epoch_count(h);
if(!frontLen||!streams||!epochs) throw Error(`index missing front=${frontLen} streams=${streams} epochs=${epochs}`);
let video=0,audio=0; while(ex.decoder_pop_video(h)>0) video++; while(ex.decoder_pop_audio(h)>0) audio++;
let expected=0n;
for(let i=0;i<2048;i++){const a=(i*17+3)&255,b=(i*29+11)&255;expected+=BigInt(Math.abs(a-b));}
const sad=ex.kernel_sad_probe(); if(BigInt(sad)!==expected) throw Error(`SAD mismatch ${sad} != ${expected}`);
if(video===0) throw Error('no video decoded');
if(ex.decoder_destroy(h)!==0) throw Error('destroy failed');
console.log(JSON.stringify({wasm:wasmPath,bytes:bytes.length,chunks,frontLen,streams,epochs,video,audio,sad:String(sad)}));
