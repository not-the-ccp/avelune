import fs from 'node:fs';

const wasmPath = process.argv[2] ?? 'web/player/avelune-scalar.wasm';
const mediaPath = process.argv[3] ?? 'web/player/demo.avl';
const {instance} = await WebAssembly.instantiate(fs.readFileSync(wasmPath), {});
const ex = instance.exports;
if (ex.avelune_abi_version() !== 0x0001_0000) throw Error(`unexpected ABI ${ex.avelune_abi_version()}`);
const handle = ex.decoder_create();
if (!handle) throw Error('decoder_create failed');

function decoderError() {
  const ptr = ex.decoder_last_error_ptr(handle), len = ex.decoder_last_error_len(handle);
  return len ? new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ptr, len)) : 'unknown decoder error';
}

try {
  const bytes = fs.readFileSync(mediaPath);
  let offset = 0, chunks = 0;
  while (offset < bytes.length) {
    const n = Math.min(bytes.length - offset, 1 + ((chunks * 7919) % 65521));
    const ptr = ex.input_reserve(handle, n);
    new Uint8Array(ex.memory.buffer, ptr, n).set(bytes.subarray(offset, offset + n));
    if (ex.decoder_push(handle, n) !== 0) throw Error(decoderError());
    offset += n; chunks++;
  }
  if (ex.decoder_finish_input(handle) !== 0) throw Error(decoderError());
  const frontLen = ex.container_front_len(handle), streams = ex.container_stream_count(handle), epochs = ex.container_epoch_count(handle);
  if (!frontLen || !streams || !epochs) throw Error(`missing index front=${frontLen} streams=${streams} epochs=${epochs}`);
  let video = 0, audio = 0;
  while (ex.decoder_pop_video(handle) > 0) video++;
  while (ex.decoder_pop_audio(handle) > 0) audio++;
  if (!video && !audio) throw Error('no media output decoded');
  console.log(JSON.stringify({wasm: wasmPath, bytes: bytes.length, chunks, frontLen, streams, epochs, video, audio}));
} finally {
  ex.decoder_destroy(handle);
}
