#!/usr/bin/env python3
"""Small deterministic media workload for CI regression artifacts.

This is intentionally *not* a model of all real media. It provides stable cross-version
signals for several orthogonal codec stresses; heterogeneous external corpora remain separate.
"""
from __future__ import annotations
import argparse, csv, hashlib, json, math, platform, re, statistics, subprocess, tempfile, time
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
PSNR_RE=re.compile(r"average:([^ ]+)")
SSIM_RE=re.compile(r"All:([0-9.]+)")

def run(cmd, capture=False):
    return subprocess.run(cmd,cwd=ROOT,check=True,text=True,stdout=subprocess.PIPE if capture else subprocess.DEVNULL,stderr=subprocess.PIPE if capture else subprocess.DEVNULL)

def y4m(path:Path, kind:str, w=96,h=64,frames=24):
    def planes(t):
        Y=bytearray(w*h); U=bytearray(w*h//4); V=bytearray(w*h//4)
        for y in range(h):
            for x in range(w):
                if kind=='cut': v=(x*3+y*5+(0 if t<frames//2 else 113))&255
                elif kind=='pan': v=((x+t*2)*7+y*3)&255
                elif kind=='ui': v=32 if ((x//12+y//8+t//6)&1)==0 else 224
                elif kind=='gradient': v=(x*255//max(1,w-1)+t)&255
                elif kind=='noise':
                    z=(x*0x9e3779b1+y*0x85ebca6b+t*0xc2b2ae35)&0xffffffff; z^=z>>16; v=z&255
                else: v=(x*11+y*17+t*19)&255
                Y[y*w+x]=v
        cw,ch=w//2,h//2
        for y in range(ch):
            for x in range(cw):
                i=y*cw+x
                if kind=='chroma': U[i]=(x*19+t*7)&255; V[i]=(y*23-t*5)&255
                else: U[i]=(96+t*2+x)&255; V[i]=(160-t*3+y)&255
        return Y,U,V
    with path.open('wb') as f:
        f.write(f'YUV4MPEG2 W{w} H{h} F24:1 Ip A1:1 C420jpeg\n'.encode())
        for t in range(frames):
            f.write(b'FRAME\n'); [f.write(p) for p in planes(t)]

def metric(src:Path, dec:Path, kind:str):
    p=run(['ffmpeg','-nostdin','-hide_banner','-i',str(src),'-i',str(dec),'-lavfi',kind,'-f','null','-'],capture=True)
    text=p.stderr
    m=(PSNR_RE if kind=='psnr' else SSIM_RE).findall(text)
    if not m: return None
    if kind=='psnr' and m[-1]=='inf': return None
    return float(m[-1])

def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()

def text(cmd):
    try: return subprocess.run(cmd,cwd=ROOT,check=True,text=True,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL).stdout.strip()
    except Exception: return None

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--cli',required=True); ap.add_argument('--out-json',required=True); ap.add_argument('--out-csv'); ap.add_argument('--q',type=int,default=96); ap.add_argument('--repeats',type=int,default=2)
    a=ap.parse_args(); cli=Path(a.cli).resolve(); rows=[]
    kinds=['cut','pan','ui','gradient','noise','chroma']
    with tempfile.TemporaryDirectory(prefix='avelune-ci-media-') as td0:
        td=Path(td0)
        for kind in kinds:
            src=td/f'{kind}.y4m'; enc=td/f'{kind}.avl'; dec=td/f'{kind}.decoded.y4m'; y4m(src,kind)
            samples=[]
            for _ in range(a.repeats):
                t=time.perf_counter(); run([str(cli),'raw','encode-y4m',str(src),str(enc),'--q',str(a.q),'--preset','balanced']); samples.append(time.perf_counter()-t)
            d_samples=[]
            for _ in range(a.repeats):
                t=time.perf_counter(); run([str(cli),'raw','decode-y4m',str(enc),str(dec)]); d_samples.append(time.perf_counter()-t)
            rows.append({'case':kind,'implementation':'canonical','q':a.q,'source_sha256':sha(src),'encoded_bytes':enc.stat().st_size,'encoded_sha256':sha(enc),'decoded_sha256':sha(dec),'psnr_db':metric(src,dec,'psnr'),'ssim':metric(src,dec,'ssim'),'encode_seconds_samples':samples,'encode_seconds_median':statistics.median(samples),'decode_seconds_samples':d_samples,'decode_seconds_median':statistics.median(d_samples)})
    commit=text(['git','rev-parse','HEAD']); dirty=bool(text(['git','status','--porcelain']))
    meta={'schema':'avelune-ci-media-v3','implementation':'canonical','q':a.q,'repeats':a.repeats,
          'provenance':{'commit':commit,'dirty':dirty,'rustc':text(['rustc','--version']),'platform':platform.platform(),'machine':platform.machine(),'cli_sha256':sha(cli)},
          'cases':rows}
    Path(a.out_json).write_text(json.dumps(meta,indent=2)+'\n')
    if a.out_csv:
        fields=['case','implementation','q','source_sha256','encoded_bytes','psnr_db','ssim','encode_seconds_median','decode_seconds_median']
        with Path(a.out_csv).open('w',newline='') as f:
            w=csv.DictWriter(f,fieldnames=fields);w.writeheader();w.writerows({k:r[k] for k in fields} for r in rows)
if __name__=='__main__': main()
