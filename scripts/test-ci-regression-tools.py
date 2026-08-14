#!/usr/bin/env python3
"""Self-test the codec regression comparator: unchanged passes, degraded fails."""
import json, subprocess, sys, tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def artifact(size=1000,psnr=40.0,ssim=.99,t=.1):
    return {'schema':'avelune-ci-media-v3','cases':[{'case':'x','source_sha256':'same','encoded_bytes':size,'psnr_db':psnr,'ssim':ssim,'encode_seconds_median':t,'decode_seconds_median':t}]}
with tempfile.TemporaryDirectory() as td0:
    td=Path(td0); base=td/'base.json'; head=td/'head.json'
    base.write_text(json.dumps(artifact())); head.write_text(json.dumps(artifact()))
    ok=subprocess.run([sys.executable,str(ROOT/'scripts/compare-ci-media.py'),str(base),str(head)],cwd=ROOT)
    if ok.returncode!=0: raise SystemExit('comparator rejected identical artifacts')
    head.write_text(json.dumps(artifact(size=1300,psnr=38.0)))
    bad=subprocess.run([sys.executable,str(ROOT/'scripts/compare-ci-media.py'),str(base),str(head)],cwd=ROOT,stdout=subprocess.DEVNULL)
    if bad.returncode==0: raise SystemExit('comparator accepted deliberately degraded artifact')
print('CI regression-tool self-test PASS')
