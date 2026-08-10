import subprocess, pathlib, time, re, csv, os, sys
ROOT=pathlib.Path(__file__).resolve().parents[1]
BIN=pathlib.Path(os.environ.get('AVELUNE_BIN', ROOT/'target/release/avelune'))
OUT=ROOT/'benchmarks/v1/runs'; OUT.mkdir(parents=True,exist_ok=True)
sources=[ROOT/'tests/corpus/v1-real/world.y4m',ROOT/'tests/corpus/v1-real/example-movie.y4m',ROOT/'tests/corpus/v1-real/lion-pan.y4m']

def run(cmd):
    t=time.perf_counter(); p=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True); dt=time.perf_counter()-t
    if p.returncode: print(p.stdout,p.stderr,file=sys.stderr); raise SystemExit('failed '+repr(cmd))
    return dt,p

def psnr(ref, test):
    p=subprocess.run(['ffmpeg','-hide_banner','-i',str(ref),'-i',str(test),'-lavfi','[0:v][1:v]psnr','-f','null','-'],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    m=re.findall(r'average:([0-9.]+|inf)',p.stderr)
    if not m: raise RuntimeError('no psnr '+p.stderr[-1000:])
    return float('inf') if m[-1]=='inf' else float(m[-1])
rows=[]
for src in sources:
    name=src.stem
    # Avelune
    for q in (64,96,128,192,256,384,512,768,1024):
        avl=OUT/f'{name}-avelune-q{q}.avl'; dec=OUT/f'{name}-avelune-q{q}.y4m'
        dt,_=run([str(BIN),'raw','encode-y4m',str(src),str(avl),'--q',str(q),'--epoch','60','--preset','balanced'])
        run([str(BIN),'raw','decode-y4m',str(avl),str(dec)])
        rows.append([name,'avelune',f'q{q}',avl.stat().st_size,dt,psnr(src,dec)])
    codecs=[
      ('x264','libx264',[20,28],['-preset','medium']),
      ('x265','libx265',[20,28],['-preset','medium','-x265-params','log-level=error']),
      ('vp9','libvpx-vp9',[30,40],['-deadline','good','-cpu-used','2','-b:v','0']),
      ('av1','libaom-av1',[30,40],['-cpu-used','6','-b:v','0']),
    ]
    for label,enc,crfs,extra in codecs:
      for crf in crfs:
        f=OUT/f'{name}-{label}-crf{crf}.mkv'
        dt,_=run(['ffmpeg','-y','-loglevel','error','-i',str(src),'-c:v',enc,'-crf',str(crf),*extra,'-an',str(f)])
        rows.append([name,label,f'crf{crf}',f.stat().st_size,dt,psnr(src,f)])
with open(ROOT/'benchmarks/v1/real-video.csv','w',newline='') as f:
    w=csv.writer(f);w.writerow(['source','codec','setting','bytes','encode_seconds','psnr_db']);w.writerows(rows)
print(open(ROOT/'benchmarks/v1/real-video.csv').read())
