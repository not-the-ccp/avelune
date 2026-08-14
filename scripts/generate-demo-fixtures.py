#!/usr/bin/env python3
"""Generate the retained synthetic motion and screen-content browser fixtures.

The source is entirely procedural and contains no third-party media. The encoded bytes are produced
by the canonical CLI with q=96 / balanced so changes are reviewable with --check.
"""
from __future__ import annotations
import argparse, subprocess, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
W, H, FPS, FRAMES = 160, 90, 30, 90

def write_y4m(path: Path, kind: str) -> None:
    with path.open('wb') as f:
        f.write(f'YUV4MPEG2 W{W} H{H} F{FPS}:1 Ip A1:1 C420jpeg\n'.encode())
        for t in range(FRAMES):
            y = bytearray(W * H)
            u = bytearray((W // 2) * (H // 2))
            v = bytearray(len(u))
            if kind == 'motion':
                for yy in range(H):
                    for xx in range(W):
                        base = 34 + ((xx * 3 + yy * 2 + t * 4) % 150)
                        if ((xx // 8) + (yy // 8) + (t // 4)) % 2 == 0:
                            base += 16
                        x1 = (t * 3) % (W + 32) - 16
                        y1 = 18 + int(10 * ((t % 30) / 29))
                        x2 = W - 1 - ((t * 2) % W)
                        if abs(xx - x1) < 13 and abs(yy - y1) < 10:
                            base = 220
                        if abs(xx - x2) < 8 and abs(yy - 62) < 8:
                            base = 65
                        y[yy * W + xx] = max(16, min(235, base))
                cw = W // 2
                for yy in range(H // 2):
                    for xx in range(cw):
                        uu = 118 + ((xx + t // 2) % 22) - 11
                        vv = 138 + ((yy + t // 3) % 18) - 9
                        x1 = ((t * 3) % (W + 32) - 16) // 2
                        y1 = (18 + int(10 * ((t % 30) / 29))) // 2
                        if abs(xx - x1) < 7 and abs(yy - y1) < 6:
                            uu, vv = 92, 190
                        u[yy * cw + xx] = max(16, min(240, uu))
                        v[yy * cw + xx] = max(16, min(240, vv))
            elif kind == 'screen':
                y[:] = bytes([225]) * len(y)
                u[:] = bytes([128]) * len(u)
                v[:] = bytes([128]) * len(v)
                def rect(x0: int, y0: int, x1: int, y1: int, value: int) -> None:
                    x0, y0, x1, y1 = max(0, x0), max(0, y0), min(W, x1), min(H, y1)
                    for row in range(y0, y1):
                        y[row * W + x0:row * W + x1] = bytes([value]) * (x1 - x0)
                rect(0, 0, W, 9, 50); rect(0, 9, 30, H, 194)
                rect(32, 11, 158, 61, 242); rect(32, 63, 158, 88, 42)
                for cx, value in ((7, 170), (14, 205), (21, 120)):
                    rect(cx - 2, 3, cx + 2, 7, value)
                for row in range(15, 82, 8):
                    rect(5, row, 22 + (row % 17), row + 2, 128)
                for i, row in enumerate(range(16, 57, 5)):
                    rect(38, row, 45, row + 2, 115)
                    rect(49, row, 78 + (i * 13) % 65, row + 2, 80 if i % 3 else 145)
                for i, row in enumerate(range(68, 85, 4)):
                    rect(38, row, 70 + (i * 17) % 75, row + 1, 185)
                caret = 48 + (t // 3) % 88
                rect(caret, 52, caret + 2, 57, 25)
                if 30 <= t < 60:
                    rect(70, 31, 116, 35, 202)
                cw = W // 2
                for yy in range(H // 2):
                    for xx in range(cw):
                        if yy < 5:
                            u[yy * cw + xx], v[yy * cw + xx] = 112, 148
                        if 26 <= yy <= 29 and abs(xx - caret // 2) < 2:
                            u[yy * cw + xx], v[yy * cw + xx] = 174, 105
            else:
                raise ValueError(kind)
            f.write(b'FRAME\n'); f.write(y); f.write(u); f.write(v)

def encode(cli: Path, source: Path, output: Path) -> None:
    subprocess.run(
        [str(cli), 'raw', 'encode-y4m', str(source), str(output), '--q', '96', '--preset', 'balanced'],
        cwd=ROOT, check=True, stdout=subprocess.DEVNULL,
    )

def generate(cli: Path, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='avelune-demo-fixtures-') as td0:
        td = Path(td0)
        for kind in ('motion', 'screen'):
            source = td / f'{kind}.y4m'
            write_y4m(source, kind)
            encode(cli, source, out / f'{kind}.avl')

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument('--cli', type=Path, default=ROOT / 'target/debug/avelune')
    ap.add_argument('--out-dir', type=Path, default=ROOT / 'web/player')
    ap.add_argument('--check', action='store_true')
    args = ap.parse_args()
    cli = args.cli.resolve()
    if not cli.exists():
        ap.error(f'CLI not found: {cli}; build avelune-cli first')
    if args.check:
        with tempfile.TemporaryDirectory(prefix='avelune-demo-check-') as td:
            generated = Path(td)
            generate(cli, generated)
            for name in ('motion.avl', 'screen.avl'):
                expected = args.out_dir / name
                if not expected.exists() or expected.read_bytes() != (generated / name).read_bytes():
                    raise SystemExit(f'{name} is stale; run {Path(__file__).name} to regenerate fixtures')
        print('demo fixture reproducibility PASS')
    else:
        generate(cli, args.out_dir)
        for name in ('motion.avl', 'screen.avl'):
            p = args.out_dir / name
            print(f'wrote {p.relative_to(ROOT)} ({p.stat().st_size} bytes)')

if __name__ == '__main__':
    main()
