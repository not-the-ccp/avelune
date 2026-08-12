#!/usr/bin/env python3
"""Compare same-runner deterministic media artifacts and fail on meaningful regressions."""
import argparse,json,math,statistics,sys
from pathlib import Path
ap=argparse.ArgumentParser();ap.add_argument('base');ap.add_argument('head');ap.add_argument('--json');a=ap.parse_args()
b=json.loads(Path(a.base).read_text());h=json.loads(Path(a.head).read_text())
B={r['case']:r for r in b['cases']};H={r['case']:r for r in h['cases']}; failures=[]; notes=[]
if set(B)!=set(H): failures.append('case set differs')
for k in sorted(set(B)&set(H)):
    x,y=B[k],H[k]
    if x['source_sha256']!=y['source_sha256']: failures.append(f'{k}: source hash differs');continue
    rate=y['encoded_bytes']/x['encoded_bytes']; psnr=(y['psnr_db'] or 999)-(x['psnr_db'] or 999); ssim=(y['ssim'] or 1)-(x['ssim'] or 1)
    # Reject uncompensated deterministic regressions; allow explicit rate/quality tradeoffs.
    if rate>1.05 and psnr<0.10 and ssim<0.0005: failures.append(f'{k}: size +{(rate-1)*100:.1f}% without quality gain')
    if psnr<-0.50 and rate>0.97: failures.append(f'{k}: PSNR {psnr:.3f} dB without >=3% rate gain')
    if ssim<-0.003 and rate>0.97: failures.append(f'{k}: SSIM {ssim:.6f} without >=3% rate gain')
    notes.append({'case':k,'size_ratio':rate,'psnr_delta_db':psnr,'ssim_delta':ssim})
enc_ratio=statistics.geometric_mean(H[k]['encode_seconds_median']/B[k]['encode_seconds_median'] for k in B if B[k]['encode_seconds_median']>0)
dec_ratio=statistics.geometric_mean(H[k]['decode_seconds_median']/B[k]['decode_seconds_median'] for k in B if B[k]['decode_seconds_median']>0)
if enc_ratio>1.75: failures.append(f'aggregate encode time catastrophic regression: {enc_ratio:.2f}x')
if dec_ratio>1.75: failures.append(f'aggregate decode time catastrophic regression: {dec_ratio:.2f}x')
out={'schema':'avelune-ci-media-comparison-v1','encode_time_ratio':enc_ratio,'decode_time_ratio':dec_ratio,'cases':notes,'failures':failures}
print(json.dumps(out,indent=2))
if a.json: Path(a.json).write_text(json.dumps(out,indent=2)+'\n')
sys.exit(1 if failures else 0)
