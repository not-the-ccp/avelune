#!/usr/bin/env python3
"""Generate larger procedural browser showcase clips with audio.

These are intentionally synthetic and self-authored: no downloaded or third-party media is used.
The script renders RGB frames with Pillow, synthesizes PCM audio, muxes a temporary MP4 with
system FFmpeg, then asks the canonical Avelune CLI to encode that ordinary media. Unlike the tiny
bit-exact regression fixtures, these are presentation/realism fixtures rather than normative vectors.
"""
from __future__ import annotations

import argparse
import math
import struct
import subprocess
import tempfile
import wave
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
W, H, FPS, SECONDS = 640, 360, 30, 4
FRAMES = FPS * SECONDS
FONT = ImageFont.load_default(size=14)
FONT_BIG = ImageFont.load_default(size=20)


def clamp(v: float, lo: int = 0, hi: int = 255) -> int:
    return max(lo, min(hi, int(v)))


def background_gradient(draw: ImageDraw.ImageDraw, top: tuple[int, int, int], bottom: tuple[int, int, int], y1: int = H) -> None:
    for y in range(y1):
        a = y / max(1, y1 - 1)
        c = tuple(round(top[i] * (1 - a) + bottom[i] * a) for i in range(3))
        draw.line((0, y, W, y), fill=c)


def frame_2d(t: int) -> Image.Image:
    im = Image.new('RGB', (W, H), (20, 24, 35)); d = ImageDraw.Draw(im)
    background_gradient(d, (23, 34, 58), (91, 126, 139), 250)
    cam = t * 4
    # parallax skyline and clouds
    for i in range(12):
        x = int((i * 89 - cam * 0.22) % (W + 120)) - 60
        bh = 35 + (i * 19) % 70
        d.rectangle((x, 250 - bh, x + 58, 250), fill=(38, 58, 72))
        for wy in range(250 - bh + 8, 244, 13):
            for wx in range(x + 8, x + 50, 14):
                if (wx + wy + i) % 3 == 0:
                    d.rectangle((wx, wy, wx + 5, wy + 4), fill=(146, 181, 160))
    for i in range(5):
        x = int((i * 177 - cam * 0.08) % (W + 160)) - 80
        y = 42 + (i * 27) % 80
        d.ellipse((x, y, x + 68, y + 24), fill=(188, 207, 214))
        d.ellipse((x + 20, y - 10, x + 52, y + 22), fill=(202, 216, 221))
    # terrain
    d.rectangle((0, 250, W, H), fill=(46, 61, 45))
    for x in range(-32 - (cam % 32), W + 32, 32):
        d.rectangle((x, 250, x + 30, 278), fill=(79, 98, 57))
        d.line((x, 279, x + 30, 279), fill=(29, 38, 30), width=2)
    # platforms
    for i in range(6):
        x = int((120 + i * 170 - cam) % (W + 260)) - 130
        y = 205 - (i % 3) * 34
        d.rectangle((x, y, x + 88, y + 10), fill=(107, 91, 61))
        d.line((x, y, x + 88, y), fill=(169, 148, 93), width=2)
    # player jump + animation
    phase = (t % 45) / 45
    px = 170
    py = 230 - int(max(0.0, math.sin(phase * math.pi)) * 72)
    d.rectangle((px - 10, py - 20, px + 10, py + 4), fill=(235, 207, 82))
    d.rectangle((px - 7, py - 28, px + 7, py - 18), fill=(232, 169, 105))
    d.rectangle((px - 11, py + 4, px - 2, py + 15), fill=(37, 43, 55))
    d.rectangle((px + 2, py + 4, px + 11, py + 15), fill=(37, 43, 55))
    # enemy + projectiles + particles
    ex = 420 - int((t * 1.7) % 100)
    ey = 230
    d.ellipse((ex - 15, ey - 15, ex + 15, ey + 15), fill=(185, 61, 71), outline=(246, 116, 92), width=3)
    for i in range(5):
        bx = int((px + 20 + ((t * 11 + i * 131) % 420)) % 640)
        by = py - 10 + ((i * 17) % 30) - 15
        d.ellipse((bx - 3, by - 3, bx + 3, by + 3), fill=(255, 239, 130))
    for i in range(16):
        age = (t + i * 7) % 30
        x = ex + int(math.cos(i * 1.7) * age * 1.4)
        y = ey + int(math.sin(i * 2.1) * age * 0.9)
        d.rectangle((x, y, x + 2, y + 2), fill=(220, 108 + (i * 9) % 80, 67))
    # HUD
    d.rounded_rectangle((18, 18, 218, 62), radius=8, fill=(12, 15, 21), outline=(94, 123, 136))
    d.text((30, 26), 'HP', font=FONT, fill=(215, 225, 225))
    d.rectangle((58, 29, 198, 39), fill=(49, 58, 62)); d.rectangle((58, 29, 185 - (t // 15) % 40, 39), fill=(92, 201, 112))
    d.text((30, 44), f'SCORE {t * 137:06d}', font=FONT, fill=(230, 212, 126))
    return im


def frame_3d(t: int) -> Image.Image:
    im = Image.new('RGB', (W, H)); d = ImageDraw.Draw(im)
    background_gradient(d, (30, 54, 91), (189, 145, 104), 190)
    horizon = 155 + int(math.sin(t / 23) * 4)
    # mountains
    pts = [(0, horizon + 25)]
    for i in range(9):
        x = i * 90 - 60
        y = horizon - 10 - ((i * 37) % 65)
        pts.extend([(x, horizon + 25), (x + 45, y), (x + 90, horizon + 25)])
    pts += [(W, horizon + 25), (W, H), (0, H)]
    d.polygon(pts, fill=(63, 66, 72))
    # ground
    d.rectangle((0, horizon, W, H), fill=(74, 69, 58))
    sway = math.sin(t / 18) * 28
    road_top_left = W / 2 - 42 + sway * .15
    road_top_right = W / 2 + 42 + sway * .15
    road_bottom_left = 78 + sway
    road_bottom_right = W - 78 + sway
    d.polygon([(road_top_left, horizon), (road_top_right, horizon), (road_bottom_right, H), (road_bottom_left, H)], fill=(48, 51, 56))
    # shoulders
    d.line((road_top_left, horizon, road_bottom_left, H), fill=(222, 215, 177), width=5)
    d.line((road_top_right, horizon, road_bottom_right, H), fill=(222, 215, 177), width=5)
    # lane markers moving toward camera
    speed = t * 0.085
    for i in range(14):
        z = ((i / 14 + speed) % 1.0)
        # perspective z: 0 horizon -> 1 camera
        p = z * z
        y = horizon + p * (H - horizon)
        half = 3 + p * 13
        center = W / 2 + sway * p
        seg = 5 + p * 34
        d.polygon([(center-half, y), (center+half, y), (center+half*1.25, y+seg), (center-half*1.25, y+seg)], fill=(230, 220, 147))
    # roadside objects with perspective motion
    for i in range(18):
        z = ((i * .071 + speed * .77) % 1.0)
        p = z * z
        side = -1 if i % 2 else 1
        y = horizon + p * (H - horizon)
        x = W/2 + side * (58 + p * 255) + sway * p
        size = 3 + p * 28
        d.rectangle((x-size*.18, y-size*1.8, x+size*.18, y), fill=(41, 44, 42))
        d.ellipse((x-size, y-size*2.8, x+size, y-size*.8), fill=(46, 96, 59))
    # opponent cars
    for i in range(4):
        z = ((i * .21 + speed * .42 + .25) % .92) + .06
        p = z * z
        y = horizon + p * (H - horizon)
        lane = (-1 if i % 2 else 1) * (12 + p * 50)
        x = W/2 + lane + sway * p
        cw, ch = 8 + p * 44, 5 + p * 30
        d.rectangle((x-cw/2, y-ch, x+cw/2, y), fill=(160 + i*17, 53 + i*19, 58 + i*11), outline=(230, 210, 190))
    # dashboard
    d.polygon([(0,H),(0,325),(120,306),(W-120,306),(W,325),(W,H)], fill=(17,20,24))
    d.ellipse((W/2-68, 302, W/2+68, 432), outline=(97,105,115), width=8)
    speed_kph = 132 + int(34 * math.sin(t/31))
    d.text((W/2-36, 322), f'{speed_kph:03d}', font=FONT_BIG, fill=(208, 231, 235))
    d.text((W/2-22, 344), 'KM/H', font=FONT, fill=(115, 145, 155))
    return im


def frame_text(t: int) -> Image.Image:
    im = Image.new('RGB', (W, H), (16, 18, 20)); d = ImageDraw.Draw(im)
    # top chrome + side bar
    d.rectangle((0,0,W,34), fill=(30,34,38)); d.rectangle((0,34,132,H), fill=(23,26,29))
    d.text((14,10),'AVELUNE LAB  ·  scene_demo.avl',font=FONT,fill=(199,210,216))
    d.text((18,49),'PROJECT',font=FONT,fill=(115,130,139))
    files=['src/codec.rs','src/player.js','demo/scene.avl','bench/results.csv','README.adoc']
    for i,name in enumerate(files):
        fill=(213,222,225) if i==(t//35)%len(files) else (133,146,154)
        d.text((22,72+i*24),name,font=FONT,fill=fill)
    # editor area
    d.rectangle((132,34,W,255), fill=(19,22,24))
    code=[
        'fn encode_frame(frame, refs) {',
        '    let prediction = motion_search(frame, refs);',
        '    let residual = transform(frame - prediction);',
        '    entropy.write(quantize(residual, q));',
        '}',
        '',
        'for epoch in stream.epochs() {',
        '    decode(epoch);',
        '    present(video, audio);',
        '}',
    ]
    scroll=(t//25)%4
    for i in range(17):
        line_no=i+1+scroll
        text=code[(i+scroll)%len(code)]
        y=48+i*12
        d.text((145,y),f'{line_no:02d}',font=FONT,fill=(67,78,84))
        col=(155,190,204) if 'fn ' in text or 'for ' in text else (196,201,194)
        d.text((174,y),text,font=FONT,fill=col)
    cursor_x=174+((t//3)%26)*7
    cursor_y=48+((t//18)%12)*12
    if (t//10)%2==0: d.rectangle((cursor_x,cursor_y,cursor_x+2,cursor_y+10),fill=(222,193,102))
    # lower terminal + chart
    d.rectangle((132,255,W,H), fill=(10,12,13)); d.line((132,255,W,255),fill=(52,58,62))
    lines=[
        '$ avelune inspect demo.avl',
        'streams: video ALV1 640x360 · audio ALA1 48000Hz stereo',
        f'frame {t:04d}  decode={(3.4+math.sin(t/8)*.7):.2f}ms  range={18+(t%7)} KiB',
        'status: playing · renderer=webgpu · wasm=simd128',
    ]
    for i,line in enumerate(lines): d.text((145,268+i*18),line,font=FONT,fill=(126,205,149) if i==0 else (164,177,181))
    # sparkline
    ox,oy=465,334
    pts=[]
    for i in range(70):
        v=math.sin((i+t)*.18)*8 + math.sin((i+t)*.047)*5
        pts.append((ox+i*2,oy-v))
    d.line(pts,fill=(98,181,223),width=2)
    return im


def synth_audio(path: Path, kind: str) -> None:
    rate=48_000; channels=2; total=SECONDS*rate
    with wave.open(str(path),'wb') as w:
        w.setnchannels(channels); w.setsampwidth(2); w.setframerate(rate)
        frames=bytearray()
        for i in range(total):
            t=i/rate
            if kind=='2d':
                tone=.24*math.sin(2*math.pi*220*t)+.13*math.sin(2*math.pi*330*t)
                beat=.22*math.sin(2*math.pi*70*t)*max(0,1-((t*2)%1)*5)
                s=tone+beat
            elif kind=='3d':
                engine=.32*math.sin(2*math.pi*(80+18*math.sin(t*.7))*t)
                whine=.11*math.sin(2*math.pi*(320+40*math.sin(t*.3))*t)
                s=engine+whine
            else:
                s=.10*math.sin(2*math.pi*440*t)
                if int(t*4)%4==0: s+=.08*math.sin(2*math.pi*880*t)
            left=clamp((s+.03*math.sin(2*math.pi*1.3*t))*28000,-32768,32767)
            right=clamp((s-.03*math.sin(2*math.pi*1.1*t))*28000,-32768,32767)
            frames += struct.pack('<hh',left,right)
        w.writeframes(frames)


def render_mp4(path: Path, kind: str) -> None:
    audio=path.with_suffix('.wav'); synth_audio(audio,kind)
    renderer={'2d':frame_2d,'3d':frame_3d,'text':frame_text}[kind]
    cmd=['ffmpeg','-hide_banner','-loglevel','error','-y','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}','-r',str(FPS),'-i','pipe:0','-i',str(audio),'-c:v','libx264','-preset','veryfast','-crf','18','-pix_fmt','yuv420p','-c:a','aac','-b:a','128k','-shortest',str(path)]
    proc=subprocess.Popen(cmd,stdin=subprocess.PIPE)
    assert proc.stdin is not None
    try:
        for i in range(FRAMES): proc.stdin.write(renderer(i).tobytes())
    finally:
        proc.stdin.close()
    if proc.wait()!=0: raise SystemExit(f'ffmpeg failed for {kind}')


def generate(cli: Path, out: Path) -> None:
    out.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='avelune-showcase-') as td0:
        td=Path(td0)
        for kind,name in [('2d','showcase-2d.avl'),('3d','showcase-3d.avl'),('text','showcase-text.avl')]:
            mp4=td/f'{kind}.mp4'; render_mp4(mp4,kind)
            subprocess.run([str(cli),'encode',str(mp4),str(out/name),'--video-q','160','--audio-q','64','--epoch','60','--preset','fast'],cwd=ROOT,check=True)
            print(f'wrote {out/name} ({(out/name).stat().st_size/1024/1024:.2f} MiB)')


def main() -> None:
    ap=argparse.ArgumentParser(); ap.add_argument('--cli',type=Path,default=ROOT/'target/debug/avelune'); ap.add_argument('--out-dir',type=Path,default=ROOT/'web/player')
    a=ap.parse_args(); generate(a.cli.resolve(),a.out_dir)

if __name__=='__main__': main()
