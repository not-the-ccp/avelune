import fs from 'node:fs';
const file=process.argv[2]||'web/player/demo.avl', wasmPath=process.argv[3]||'web/player/avelune.wasm';
const b=fs.readFileSync(file), w=fs.readFileSync(wasmPath), {instance}=await WebAssembly.instantiate(w,{}), ex=instance.exports;
const dv=(buf,off=0,len=buf.length-off)=>new DataView(buf.buffer,buf.byteOffset+off,len); const u16=(v,o)=>v.getUint16(o,true),u32=(v,o)=>v.getUint32(o,true),u64=(v,o)=>Number(v.getBigUint64(o,true));
let requests=[]; function range(a,z){requests.push([a,z]); return b.subarray(a,z+1)}
let h=range(0,31), hv=dv(h); if(h.toString('ascii',0,7)!=='AVELUNE')throw Error('magic'); let fl=u32(hv,24), f=range(32,31+fl), fv=dv(f), sc=u16(fv,6),ec=u32(fv,8),p=12; p+=sc*24; let eps=[]; for(let i=0;i<ec;i++){eps.push({id:u32(fv,p),dur:u32(fv,p+4),pts:u64(fv,p+8),off:u64(fv,p+16),len:u64(fv,p+24)});p+=32}
function feed(payload,kind){let ptr=ex.avelune_input_resize(payload.length);new Uint8Array(ex.memory.buffer,ptr,payload.length).set(payload);return kind===2?ex.avelune_decode_video_input(payload.length):ex.avelune_decode_audio_input(payload.length)}
let videos=0,audioFrames=0;
for(const e of eps){let x=range(e.off,e.off+e.len-1),q=0;ex.avelune_reset();while(q<x.length){let v=dv(x,q,28),kind=x[q+4],len=u32(v,20),payload=x.subarray(q+28,q+28+len);if(kind===2){if(feed(payload,2)!==0)throw Error('video '+ex.avelune_last_error());videos++}else if(kind===3){if(feed(payload,3)!==0)throw Error('audio '+ex.avelune_last_error());audioFrames+=ex.avelune_audio_len_samples()/ex.avelune_audio_channels()}q+=28+len+4}}
console.log(JSON.stringify({fileBytes:b.length,frontBytes:32+fl,epochs:eps.length,videoFrames:videos,audioSampleFrames:audioFrames,requests},null,2));
