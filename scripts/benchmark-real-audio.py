import subprocess, pathlib, time, csv, math, struct, wave, os
ROOT=pathlib.Path(__file__).resolve().parents[1]
BIN=pathlib.Path(os.environ.get('AVELUNE_BIN', ROOT/'target/release/avelune'))
OUT=ROOT/'benchmarks/v1/audio-runs';OUT.mkdir(parents=True,exist_ok=True)
MEDIA_ROOT=pathlib.Path(os.environ.get('AVELUNE_BENCHMARK_AUDIO_ROOT', ROOT/'tests/corpus/v1-real/audio'))
SOURCES={
 'sax':MEDIA_ROOT/'sax.wav',
 'cantina':MEDIA_ROOT/'cantina.wav',
 'speech-heath':MEDIA_ROOT/'heath_ledger.mp3',
 'speech-cate':MEDIA_ROOT/'cate_blanch.mp3',
}
def run(cmd):
 t=time.perf_counter();p=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE);dt=time.perf_counter()-t
 if p.returncode: raise RuntimeError((cmd,p.stderr[-2000:]))
 return dt

def pcm(path):
 b=path.read_bytes();return struct.unpack('<%dh'%(len(b)//2),b)
def snr(a,b):
 n=min(len(a),len(b));sig=sum(float(x)*x for x in a[:n]);err=sum(float(a[i]-b[i])**2 for i in range(n));return float('inf') if err==0 else 10*math.log10(max(sig,1)/err)
rows=[]
for name,src in SOURCES.items():
 ref=OUT/f'{name}.s16';run(['ffmpeg','-y','-loglevel','error','-t','4','-i',str(src),'-ar','48000','-ac','2','-f','s16le',str(ref)])
 ra=pcm(ref)
 for q in (64,128,256,512,1):
  f=OUT/f'{name}-avelune-q{q}.avl';dec=OUT/f'{name}-avelune-q{q}.s16'
  dt=run([str(BIN),'encode-audio',str(src),str(f),'--seconds','4','--q',str(q)])
  run([str(BIN),'decode-audio',str(f),str(dec)])
  rows.append([name,'avelune',f'q{q}',f.stat().st_size,dt,snr(ra,pcm(dec))])
 for kb in (32,64,96,128):
  f=OUT/f'{name}-opus-{kb}k.ogg';dec=OUT/f'{name}-opus-{kb}k.s16'
  dt=run(['ffmpeg','-y','-loglevel','error','-t','4','-i',str(src),'-ar','48000','-ac','2','-c:a','libopus','-b:a',f'{kb}k',str(f)])
  run(['ffmpeg','-y','-loglevel','error','-i',str(f),'-ar','48000','-ac','2','-f','s16le',str(dec)])
  rows.append([name,'opus',f'{kb}k',f.stat().st_size,dt,snr(ra,pcm(dec))])
with open(ROOT/'benchmarks/v1/real-audio.csv','w',newline='') as f:
 w=csv.writer(f);w.writerow(['source','codec','setting','bytes','encode_seconds','sample_snr_db']);w.writerows(rows)
print((ROOT/'benchmarks/v1/real-audio.csv').read_text())
