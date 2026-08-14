import fs from 'node:fs';
import {spawn} from 'node:child_process';

const media = fs.readFileSync(process.argv[2] || 'web/player/demo.avl');
const artifacts = [
  ['scalar', fs.readFileSync('web/player/avelune-scalar.wasm')],
  ['simd128', fs.readFileSync('web/player/avelune-simd128.wasm')],
];
const chromium = process.env.CHROMIUM || '/usr/bin/chromium';
const debugPort = 19223 + (process.pid % 1000);
const profile = `/tmp/avelune-chromium-${process.pid}`;
const child = spawn(chromium, [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage',
  `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profile}`, 'about:blank',
], {stdio: ['ignore', 'ignore', 'pipe']});
let stderr = '';
child.stderr.on('data', b => stderr += b);

async function target() {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
      const page = list.find(x => x.type === 'page');
      if (page?.webSocketDebuggerUrl) return page;
    } catch {}
    await new Promise(r => setTimeout(r, 50));
  }
  throw Error(`Chromium DevTools target unavailable: ${stderr.slice(-2000)}`);
}

async function cdpEvaluate(wsUrl, expression) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, {once: true});
    ws.addEventListener('error', reject, {once: true});
  });
  const id = Math.floor(Math.random() * 1_000_000_000) + 1;
  const result = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(Error('CDP evaluation timeout')), 30000);
    ws.addEventListener('message', event => {
      const msg = JSON.parse(event.data);
      if (msg.id !== id) return;
      clearTimeout(timer);
      if (msg.result?.exceptionDetails) {
        reject(Error(msg.result.exceptionDetails.exception?.description || msg.result.exceptionDetails.text));
      } else {
        resolve(msg.result?.result?.value);
      }
    });
    ws.send(JSON.stringify({
      id,
      method: 'Runtime.evaluate',
      params: {expression, awaitPromise: true, returnByValue: true},
    }));
  });
  ws.close();
  return result;
}

function browserProgram(wasmBytes) {
  const wb64 = wasmBytes.toString('base64');
  const mb64 = media.toString('base64');
  return `(async()=>{
    const bytes=s=>Uint8Array.from(atob(s),c=>c.charCodeAt(0));
    const wasm=bytes(${JSON.stringify(wb64)}), media=bytes(${JSON.stringify(mb64)});
    if(!WebAssembly.validate(wasm)) throw Error('WebAssembly.validate failed');
    const {instance}=await WebAssembly.instantiate(wasm,{}), ex=instance.exports;
    const h=ex.decoder_create(); if(!h) throw Error('decoder_create failed');
    let off=0,chunks=0;
    while(off<media.length){
      const n=Math.min(media.length-off,1+((chunks*37)%4093));
      const p=ex.input_reserve(h,n); if(!p) throw Error('input_reserve failed');
      new Uint8Array(ex.memory.buffer,p,n).set(media.subarray(off,off+n));
      if(ex.decoder_push(h,n)!==0){
        const ep=ex.decoder_last_error_ptr(h), en=ex.decoder_last_error_len(h);
        throw Error(new TextDecoder().decode(new Uint8Array(ex.memory.buffer,ep,en)));
      }
      off+=n; chunks++;
    }
    if(ex.decoder_finish_input(h)!==0){const ep=ex.decoder_last_error_ptr(h),en=ex.decoder_last_error_len(h);throw Error(new TextDecoder().decode(new Uint8Array(ex.memory.buffer,ep,en)));}
    let video=0,audio=0;
    while(ex.decoder_pop_video(h)>0)video++;
    while(ex.decoder_pop_audio(h)>0)audio++;
    const sad=ex.kernel_sad_probe();
    const half=[];
    for(let phase=0;phase<4;phase++){
      const fx=phase&1,fy=(phase>>1)&1,ref=new Uint8Array(16*9),src=new Uint8Array(16*8);
      for(let i=0;i<ref.length;i++)ref[i]=(i*31+7)&255; for(let i=0;i<src.length;i++)src[i]=(i*13+19)&255;
      let checksum=0n,hsad=0n;
      for(let y=0;y<8;y++)for(let x=0;x<8;x++){
        const a=ref[y*16+x],b=ref[y*16+x+fx],c=ref[(y+fy)*16+x],d=ref[(y+fy)*16+x+fx];
        const pv=fx===0&&fy===0?a:fx===1&&fy===0?((a+b+1)>>1):fx===0&&fy===1?((a+c+1)>>1):((a+b+c+d+2)>>2);
        const i=y*8+x;checksum+=BigInt(i+1)*BigInt(pv);hsad+=BigInt(Math.abs(src[y*16+x]-pv));
      }
      const gotP=BigInt(ex.kernel_halfpel_predict_probe(phase)),gotS=BigInt(ex.kernel_halfpel_sad_probe(phase));
      if(gotP!==checksum||gotS!==hsad)throw Error('halfpel phase '+phase+' mismatch');
      half.push([String(gotP),String(gotS)]);
    }
    const abi=ex.avelune_abi_version();
    if(abi!==0x00020000) throw Error('unexpected ABI '+abi);
    if(ex.decoder_destroy(h)!==0) throw Error('decoder_destroy failed');
    if(video!==60||audio!==100) throw Error('unexpected decoded counts '+video+'/'+audio);

    const ew=32,eh=24,eframes=5;
    const enc=ex.video_encoder_create(ew,eh,(30<<16)|1,96,1,3,2<<13);
    if(!enc) throw Error('video_encoder_create failed in Chromium');
    const frameLen=ex.video_encoder_frame_len(enc);
    for(let t=0;t<eframes;t++){
      const p=ex.video_encoder_frame_ptr(enc), frame=new Uint8Array(ex.memory.buffer,p,frameLen);
      const yl=ew*eh,cl=yl/4;
      for(let y=0;y<eh;y++)for(let x=0;x<ew;x++)frame[y*ew+x]=(x*7+y*5+t*13)&255;
      frame.fill(100+t,yl,yl+cl); frame.fill(150-t,yl+cl);
      if(ex.video_encoder_push_frame(enc)!==0)throw Error('video encoder push failed');
    }
    if(ex.video_encoder_finish(enc)!==0)throw Error('video encoder finish failed');
    const encLen=ex.video_encoder_output_len(enc),encPtr=ex.video_encoder_output_ptr(enc);
    const encoded=new Uint8Array(ex.memory.buffer,encPtr,encLen).slice();
    if(ex.video_encoder_destroy(enc)!==0)throw Error('video_encoder_destroy failed');
    const h2=ex.decoder_create(); if(!h2)throw Error('roundtrip decoder_create failed');
    const p2=ex.input_reserve(h2,encoded.length); new Uint8Array(ex.memory.buffer,p2,encoded.length).set(encoded);
    if(ex.decoder_push(h2,encoded.length)!==0||ex.decoder_finish_input(h2)!==0)throw Error('encoded Chromium roundtrip decode failed');
    let encodedFrames=0; while(ex.decoder_pop_video(h2)>0)encodedFrames++;
    if(ex.decoder_destroy(h2)!==0)throw Error('roundtrip decoder_destroy failed');
    if(encodedFrames!==eframes)throw Error('Chromium encoder roundtrip count '+encodedFrames);
    return {abi,bytes:media.length,chunks,video,audio,sad:String(sad),half,encoderBytes:encLen,encoderFrames:encodedFrames};
  })()`;
}

let page;
try {
  page = await target();
  const results = {};
  for (const [name, wasm] of artifacts) {
    results[name] = await cdpEvaluate(page.webSocketDebuggerUrl, browserProgram(wasm));
  }
  if (results.scalar.sad !== results.simd128.sad) throw Error('scalar/SIMD SAD probe differs');
  if (JSON.stringify(results.scalar.half)!==JSON.stringify(results.simd128.half)) throw Error('scalar/SIMD halfpel probes differ');
  if (results.scalar.encoderBytes !== results.simd128.encoderBytes || results.scalar.encoderFrames !== results.simd128.encoderFrames) throw Error('scalar/SIMD browser encoder roundtrip differs');
  console.log(JSON.stringify({chromium: true, results}));
} finally {
  child.kill('SIGTERM');
  await new Promise(resolve => {
    const timer = setTimeout(resolve, 3000);
    child.once('close', () => { clearTimeout(timer); resolve(); });
  });
  fs.rmSync(profile, {recursive: true, force: true});
}
