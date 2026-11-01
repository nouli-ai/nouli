#!/usr/bin/env python3
"""Render recorded native TUI cells to GIF, PNG and WebM. Needs Pillow and FFmpeg.

python scripts/render-tui-media.py --replay src/data/cli/tui.json \
  --output ../.github/readme --video public/media/tui-workshop.webm --ffmpeg PATH
No inference or terminal interaction occurs during rendering.
"""
import argparse
import hashlib
import io
import json
import math
from pathlib import Path
import subprocess

from PIL import Image, ImageDraw, ImageFont

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--replay', required=True, type=Path)
parser.add_argument('--output', type=Path)
parser.add_argument('--video', required=True, type=Path)
parser.add_argument('--ffmpeg', required=True)
parser.add_argument('--speed', type=float, default=3)
parser.add_argument('--start-seconds', type=float, default=0)
parser.add_argument('--font', default='/System/Library/Fonts/Menlo.ttc')
args = parser.parse_args()
replay = json.loads(args.replay.read_text())
rate, fps = args.speed, 8
cell, line, padding = 9, 18, 12
regular = ImageFont.truetype(args.font, 14, index=0)
try:
    bold = ImageFont.truetype(args.font, 14, index=1)
except OSError:
    bold = regular
background, foreground = '#15191d', '#d1d5db'
colors = dict(black=background, red='#ef4444', green='#22c55e', brown='#eab308',
              blue='#3b82f6', magenta='#ec4899', cyan='#06b6d4', white=foreground,
              brightblack='#737373', brightwhite='#ffffff')

def color(value, default):
    if value == 'default':
        return default
    if len(value) == 6 and all(c in '0123456789abcdefABCDEF' for c in value):
        return '#' + value
    return colors.get(value, default)

def render(rows):
    image = Image.new('RGB', (replay['columns'] * cell + padding * 2,
                              replay['lines'] * line + padding * 2), background)
    draw = ImageDraw.Draw(image)
    for row, runs in enumerate(rows):
        column = 0
        for text, fg, bg, heavy, reverse in runs:
            ink, paper = color(fg, foreground), color(bg, background)
            if reverse:
                ink, paper = paper, ink
            for character in text:
                x, y = padding + column * cell, padding + row * line
                if paper != background:
                    draw.rectangle((x, y, x + cell - 1, y + line - 1), fill=paper)
                draw.text((x, y), character, font=bold if heavy else regular, fill=ink)
                column += 1
    return image

rows = [list(row) for row in replay['initial']['rows']]
updates = replay['updates']
duration = sum(update['wait'] for update in updates) / 1000 / rate - args.start_seconds
assert duration > 0 and rate > 0
samples = math.ceil(duration * fps)
args.video.parent.mkdir(parents=True, exist_ok=True)
process = subprocess.Popen([args.ffmpeg, '-y', '-hide_banner', '-loglevel', 'error',
    '-f', 'image2pipe', '-vcodec', 'png', '-framerate', str(fps), '-i', '-',
    '-an', '-c:v', 'libvpx-vp9', '-b:v', '0', '-crf', '30', '-pix_fmt', 'yuv420p',
    str(args.video)], stdin=subprocess.PIPE)
frames, index, due = [], 0, 0
try:
    for sample in range(samples + fps):
        at = (args.start_seconds + sample / fps) * rate * 1000
        while index < len(updates) and due + updates[index]['wait'] <= at:
            update = updates[index]
            due += update['wait']
            for row, runs in update['rows']:
                rows[row] = runs
            index += 1
        image = render(rows)
        data = io.BytesIO()
        image.save(data, format='PNG')
        process.stdin.write(data.getvalue())
        if args.output:
            frames.append(image.quantize(colors=128))
finally:
    process.stdin.close()
assert process.wait() == 0, 'Video encoding failed'
poster = render(replay['final']['rows'])
if args.output:
    args.output.mkdir(parents=True, exist_ok=True)
    poster.save(args.output / 'tui-poster.png')
    frames[0].save(args.output / 'tui-replay.gif', save_all=True, append_images=frames[1:],
                   duration=round(1000 / fps), loop=0, optimize=True, disposal=1)
    provenance = dict(application=replay['application'],
        capture='Native PTY recording rendered from terminal cells',
        answer_source='site/src/data/cli/graph.json', live_provider_calls=0,
        local_fixture_calls=len(replay['steps']), playback_speed=rate,
        frames_per_second=fps, final_hold_seconds=1,
        source_sha256=hashlib.sha256(args.replay.read_bytes()).hexdigest(),
        source_recording=str(args.replay),
        renderer='site/scripts/render-tui-media.py',
        dimensions=dict(columns=replay['columns'], lines=replay['lines'], width=poster.width, height=poster.height),
        recorded_at=json.loads((args.replay.parent / 'provenance.json').read_text())['wide']['recorded_at'],
        binary_sha256=json.loads((args.replay.parent / 'provenance.json').read_text())['wide']['binary_sha256'],
        media={name: dict(bytes=(args.output / name).stat().st_size,
                         sha256=hashlib.sha256((args.output / name).read_bytes()).hexdigest())
               for name in ['tui-replay.gif', 'tui-poster.png']})
    (args.output / 'tui-replay-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
print(json.dumps(dict(video=str(args.video), seconds=round(duration + 1, 2),
                      frames=samples + fps, application=replay['application'])))
