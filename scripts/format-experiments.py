#!/usr/bin/env python3
"""Measured Draft Generation format proxies used to decide whether syntax experiments deserve work.

These are deliberately labeled proxies: they never rewrite the normative bitstream and they do not
turn a promising local result into a format decision. The intent is to replace unsupported opinions
with reproducible measurements cheaply enough to run in the canonical validation workflow.
"""
from __future__ import annotations
import argparse, json, math, os, subprocess, tempfile
from pathlib import Path

BLOCK=8

def parse_y4m(path: Path):
    b=path.read_bytes(); nl=b.find(b'\n')
    if nl<0 or not b[:nl].startswith(b'YUV4MPEG2 '): raise ValueError('bad y4m')
    fields=b[:nl].decode().split()
    w=int(next(x[1:] for x in fields if x.startswith('W'))); h=int(next(x[1:] for x in fields if x.startswith('H')))
    ysz=w*h; csz=ysz//4; pos=nl+1; frames=[]
    while pos<len(b):
        e=b.find(b'\n',pos)
        if e<0 or not b[pos:e].startswith(b'FRAME'): raise ValueError('bad frame header')
        pos=e+1
        if pos+ysz+2*csz>len(b): raise ValueError('truncated frame')
        frames.append(b[pos:pos+ysz]); pos+=ysz+2*csz
    return w,h,frames

def hadamard8(v):
    v=list(v); h=1
    while h<8:
        for i in range(0,8,h*2):
            for j in range(i,i+h):
                a,b=v[j],v[j+h]; v[j]=a+b; v[j+h]=a-b
        h*=2
    return v

def wht2(a):
    a=list(a)
    for y in range(8): a[y*8:y*8+8]=hadamard8(a[y*8:y*8+8])
    for x in range(8):
        c=hadamard8([a[y*8+x] for y in range(8)])
        for y in range(8): a[y*8+x]=c[y]
    return a

def div_round(v,d): return (v+d//2)//d if v>=0 else -((-v+d//2)//d)
def inv_wht2(a):
    a=list(a)
    for y in range(8): a[y*8:y*8+8]=hadamard8(a[y*8:y*8+8])
    for x in range(8):
        c=hadamard8([a[y*8+x] for y in range(8)])
        for y in range(8): a[y*8+x]=div_round(c[y],64)
    return a

def uvarint_len(v):
    n=1
    while v>=128: v >>=7; n+=1
    return n

def svarint_len(v):
    z=(v<<1) if v>=0 else ((-v)<<1)-1
    return uvarint_len(z)

def sparse_cost(qc):
    nz=[(i,v) for i,v in enumerate(qc) if v]
    return uvarint_len(len(nz))+sum(uvarint_len(i)+svarint_len(v) for i,v in nz)

def residual_blocks(frames,w,h,max_frames=20):
    out=[]
    for fi in range(1,min(len(frames),max_frames)):
        a,b=frames[fi],frames[fi-1]
        for by in range(0,h-h%BLOCK,BLOCK):
            for bx in range(0,w-w%BLOCK,BLOCK):
                out.append([a[(by+y)*w+bx+x]-b[(by+y)*w+bx+x] for y in range(8) for x in range(8)])
    return out

def transform_skip_proxy(blocks,q_wht):
    wht_bytes=wht_sse=0
    transformed=[]
    for r in blocks:
        c=wht2(r); qc=[div_round(v,q_wht) for v in c]; rr=inv_wht2([v*q_wht for v in qc])
        wht_bytes+=sparse_cost(qc); wht_sse+=sum((x-y)**2 for x,y in zip(r,rr)); transformed.append(r)
    # Search skip quantizers for the closest distortion rather than pretending qstep has the same scale.
    candidates=[]
    for q in range(1,33):
        bs=se=0
        for r in transformed:
            qc=[div_round(v,q) for v in r]; rr=[v*q for v in qc]
            bs+=sparse_cost(qc); se+=sum((x-y)**2 for x,y in zip(r,rr))
        candidates.append((abs(se-wht_sse),q,bs,se))
    _,q,bs,se=min(candidates)
    return {
        'blocks':len(blocks),'wht_qstep':q_wht,'wht_sparse_token_bytes':wht_bytes,'wht_sse':wht_sse,
        'matched_skip_qstep':q,'skip_sparse_token_bytes':bs,'skip_sse':se,
        'skip_rate_delta_bytes':bs-wht_bytes,
        'skip_rate_delta_pct':((bs/wht_bytes)-1)*100 if wht_bytes else 0,
        'distortion_delta_pct':((se/wht_sse)-1)*100 if wht_sse else 0,
    }

def block_bytes(frame,w,h,bx,by):
    return bytes(frame[(by+y)*w+bx:(by+y)*w+bx+8] for y in [])

def blocks8(frame,w,h):
    for by in range(0,h-h%8,8):
        for bx in range(0,w-w%8,8):
            yield bx,by,bytes(frame[(by+y)*w+bx+x] for y in range(8) for x in range(8))

def palette_cost(block):
    colors=[]; idx=[]
    for v in block:
        if v not in colors:
            if len(colors)==4:return None
            colors.append(v)
        idx.append(colors.index(v))
    # split-lane data bytes: color count, colors, index count, packed 2-bit indices. Control byte is common.
    return 1+len(colors)+1+math.ceil(len(idx)/4)

def ibc_proxy(frame,w,h):
    seen={}; total=repeated=palette_repeated=0; pal_bytes=copy_bytes=0
    for bx,by,b in blocks8(frame,w,h):
        total+=1
        prev=seen.get(b)
        if prev is not None:
            repeated+=1
            pc=palette_cost(b)
            if pc is not None:
                palette_repeated+=1; pal_bytes+=pc
                dx=(prev[0]-bx)//8; dy=(prev[1]-by)//8
                # Hypothetical data: two signed block offsets. One control symbol is common to both modes.
                copy_bytes+=svarint_len(dx)+svarint_len(dy)
        else: seen[b]=(bx,by)
    return {'blocks':total,'exact_prior_block_matches':repeated,'match_pct':100*repeated/total if total else 0,
            'repeated_palette_blocks':palette_repeated,'palette_data_bytes_for_matched_subset':pal_bytes,
            'hypothetical_ibc_offset_bytes_for_same_subset':copy_bytes,
            'subset_delta_bytes':copy_bytes-pal_bytes}

def synthetic_ui(w=640,h=360):
    f=bytearray([28])*(w*h)
    # Repeated panels and 8x8 glyph/tile motifs; intentionally screen-like, not a natural-image surrogate.
    for y in range(h):
        for x in range(w):
            panel=((x//160)+(y//90))%4
            base=(40,72,104,136)[panel]
            tile=((x//8)^(y//8))&3
            v=base if tile<2 else min(255,base+18)
            if (x%40 in (5,6,7)) or (y%32 in (9,10)): v=220
            f[y*w+x]=v
    return f

def sad_block_mv(cur, ref, w, h, bx, by, dx, dy, size=8):
    sx=bx+dx; sy=by+dy
    if sx<0 or sy<0 or sx+size>w or sy+size>h: return None
    sad=0
    for y in range(size):
        co=(by+y)*w+bx; ro=(sy+y)*w+sx
        for x in range(size): sad += abs(cur[co+x]-ref[ro+x])
    return sad

def best_integer_mv(cur, ref, w, h, bx, by, radius=2, size=8):
    best=None
    for dy in range(-radius,radius+1):
        for dx in range(-radius,radius+1):
            sad=sad_block_mv(cur,ref,w,h,bx,by,dx,dy,size)
            if sad is not None and (best is None or sad<best[0]): best=(sad,dx,dy)
    return best

def partition_and_lattice_proxy(frames,w,h,max_pairs=8,radius=2):
    groups=[]; mv_current=mv_lattice=0; correction_groups=0
    pairs=min(len(frames)-1,max_pairs)
    for fi in range(1,pairs+1):
        cur,ref=frames[fi],frames[fi-1]
        for by in range(0,h-h%16,16):
            for bx in range(0,w-w%16,16):
                children=[]
                for oy in (0,8):
                    for ox in (0,8):
                        v=best_integer_mv(cur,ref,w,h,bx+ox,by+oy,radius,8)
                        if v is None: break
                        children.append(v)
                if len(children)!=4: continue
                child_sad=sum(v[0] for v in children)
                # Current ALV1-style per-block integer vectors are expressed in half-pel units.
                current=sum(svarint_len(dx*2)+svarint_len(dy*2) for _,dx,dy in children)
                mv_current+=current
                # Hypothetical 2x2 lattice cell: one base vector, one correction mask, and
                # signed local corrections only for blocks differing from the modal base.
                counts={}
                for _,dx,dy in children: counts[(dx,dy)]=counts.get((dx,dy),0)+1
                base=max(counts,key=lambda k:(counts[k],-abs(k[0])-abs(k[1])))
                differing=[(dx-base[0],dy-base[1]) for _,dx,dy in children if (dx,dy)!=base]
                lattice=svarint_len(base[0]*2)+svarint_len(base[1]*2)+1
                lattice+=sum(svarint_len(dx*2)+svarint_len(dy*2) for dx,dy in differing)
                mv_lattice+=lattice
                correction_groups += bool(differing)
                # Constrained 16x16 merge: one integer vector for all four children. This keeps
                # residual/transform details outside the proxy and directly measures prediction
                # penalty versus four independently chosen vectors.
                merged=best_integer_mv(cur,ref,w,h,bx,by,radius,16)
                if merged is not None:
                    groups.append((child_sad,merged[0],current,svarint_len(merged[1]*2)+svarint_len(merged[2]*2)))
    thresholds=[]
    for per_pixel in (0,1,2,4):
        accepted=[]
        for child,merged,cur_bytes,merge_bytes in groups:
            if merged-child <= per_pixel*16*16: accepted.append((child,merged,cur_bytes,merge_bytes))
        thresholds.append({
            'max_extra_sad_per_pixel':per_pixel,
            'accepted_groups':len(accepted),
            'accepted_pct':100*len(accepted)/len(groups) if groups else 0,
            'extra_sad':sum(m-c for c,m,_,_ in accepted),
            'motion_vector_bytes_before':sum(cb for _,_,cb,_ in accepted),
            'motion_vector_bytes_after':sum(mb for _,_,_,mb in accepted),
        })
    return {
        'sampled_frame_pairs':pairs,'radius_pixels':radius,'groups_16x16':len(groups),
        'coarse_lattice_local_correction':{
            'motion_vector_bytes_before':mv_current,'proxy_bytes_after':mv_lattice,
            'delta_bytes':mv_lattice-mv_current,
            'delta_pct':100*(mv_lattice/mv_current-1) if mv_current else 0,
            'groups_requiring_local_corrections':correction_groups,
        },
        'constrained_16x16_motion_merge':thresholds,
        'methodology':'Prediction-only proxy; residual syntax/search complexity is intentionally excluded, so this cannot by itself justify recursive partition syntax.'
    }

def luma_sse(a,b):
    return sum((x-y)*(x-y) for fa,fb in zip(a,b) for x,y in zip(fa,fb))

def deblock_proxy(source,reconstructed,w,h):
    base=luma_sse(source,reconstructed); variants=[]
    for threshold in (2,4,8,16,32):
        filtered=[]
        for fr in reconstructed:
            out=bytearray(fr)
            for y in range(h):
                for x in range(8,w,8):
                    i=y*w+x; left=fr[i-1]; right=fr[i]
                    if abs(left-right)<=threshold:
                        out[i-1]=(3*left+right+2)//4; out[i]=(left+3*right+2)//4
            tmp=bytearray(out)
            for y in range(8,h,8):
                for x in range(w):
                    i=y*w+x; top=out[i-w]; bottom=out[i]
                    if abs(top-bottom)<=threshold:
                        tmp[i-w]=(3*top+bottom+2)//4; tmp[i]=(top+3*bottom+2)//4
            filtered.append(tmp)
        err=luma_sse(source,filtered)
        variants.append({'edge_threshold':threshold,'luma_sse':err,'sse_delta_pct':100*(err/base-1) if base else 0})
    return {
        'baseline_luma_sse':base,'variants':variants,
        'methodology':'One deliberately simple 8x8-boundary [3,1]/4 reconstruction filter proxy. Positive SSE delta is a rejection signal, not evidence against every possible filter.'
    }

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--media',default='web/player/demo.avl'); ap.add_argument('--out',default='results/format-experiments.json'); ap.add_argument('--bin',default=os.environ.get('AVELUNE_BIN','target/release/avelune'))
    a=ap.parse_args(); out=Path(a.out); out.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='avelune-format-exp-') as td:
        y4m=Path(td)/'demo.y4m'
        q96=Path(td)/'q96.avl'; q96_y4m=Path(td)/'q96.y4m'
        subprocess.run([a.bin,'raw','decode-y4m',a.media,str(y4m)],check=True,stdout=subprocess.DEVNULL)
        w,h,frames=parse_y4m(y4m)
        subprocess.run([a.bin,'raw','encode-y4m','-q','96',str(y4m),str(q96)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        subprocess.run([a.bin,'raw','decode-y4m',str(q96),str(q96_y4m)],check=True,stdout=subprocess.DEVNULL)
        qw,qh,q96_frames=parse_y4m(q96_y4m)
        if (qw,qh)!=(w,h): raise ValueError('q96 reconstruction shape mismatch')
        filter_result=deblock_proxy(frames,q96_frames,w,h)
    residuals=residual_blocks(frames,w,h)
    motion_result=partition_and_lattice_proxy(frames,w,h)
    results={
      'methodology_note':'Measured proxies only; no normative syntax change is accepted by this script.',
      'source':{'media':a.media,'width':w,'height':h,'frames':len(frames),'inter_residual_blocks_sampled':len(residuals)},
      'transform_skip':[transform_skip_proxy(residuals,q) for q in (48,96,192)],
      'intra_block_copy':{
         'demo_first_frame':ibc_proxy(frames[0],w,h),
         'synthetic_ui_640x360':ibc_proxy(synthetic_ui(),640,360),
      },
      'motion_and_partition':motion_result,
      'single_reconstruction_filter':filter_result,
    }
    out.write_text(json.dumps(results,indent=2)+'\n'); print(json.dumps(results,indent=2))
if __name__=='__main__': main()
