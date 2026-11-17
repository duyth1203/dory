"""Builds the eleven turn-switch clips for the bilingual bench from the library's own takes.

usage: python3 -I scripts/bilingual_turns.py <data dir> <out dir>
       python3 -I scripts/bilingual_turns.py --list

Each clip cuts 16 kHz mono WAV from "Pulong sa Tagalog" (FLEURS read Tagalog, no English) and
"Weekly sync" (English), one turn after another with 400 ms of silence between. Six clips hold one
language. Five switch language between turns. Writes `<name>.wav`, `refs.tsv` (file, reference
text), and `langs.tsv` (file, then one `<tag>:<start ms>-<end ms>:<words>` part per turn) into
<out dir>. The reference text is the library's transcript of the take, so it is close but not
hand-checked. A run does nothing when every output already exists.
"""
import os
import sqlite3
import subprocess
import sys
import tempfile
import wave

SOURCES = {'tl': 'Pulong sa Tagalog', 'en': 'Weekly sync'}
GAP_MS = 400
BYTES_PER_MS = 32

CLIPS = {
    'pure_tl0': [('tl', 0, 24960)],
    'pure_tl1': [('tl', 26850, 47730)],
    'pure_tl2': [('tl', 50070, 71430)],
    'pure_tl3': [('tl', 72000, 89220)],
    'pure_en0': [('en', 0, 21440)],
    'pure_en1': [('en', 21440, 41120)],
    'mix_tl3_en0': [('tl', 72000, 89220), ('en', 0, 10000)],
    'mix_en2_tl3': [('en', 21440, 32080), ('tl', 72000, 89220)],
    'mix_tl1_en3': [('tl', 26850, 47730), ('en', 32480, 41120)],
    'mix_en1_tl3': [('en', 5040, 13600), ('tl', 72000, 89220)],
    'mix_en_tl3_en': [('en', 0, 5040), ('tl', 72000, 89220), ('en', 37360, 41120)],
}


def outputs(out_dir):
    return [os.path.join(out_dir, f'{name}.wav') for name in CLIPS] + [
        os.path.join(out_dir, 'refs.tsv'),
        os.path.join(out_dir, 'langs.tsv'),
    ]


def load_source(data_dir, language, scratch):
    """The take's 16 kHz mono PCM and its transcript segments as (start ms, end ms, text)."""
    title = SOURCES[language]
    db = sqlite3.connect(f'file:{os.path.join(data_dir, "dory.sqlite")}?mode=ro', uri=True)
    row = db.execute(
        'select id from recordings where title = ? order by created_at, id limit 1', (title,)
    ).fetchone()
    if row is None:
        sys.exit(f'bilingual_turns: no recording titled "{title}" in {data_dir}')
    rid = row[0]
    segments = db.execute(
        'select start_ms, end_ms, text from segments where recording_id = ? order by idx', (rid,)
    ).fetchall()
    take = os.path.join(data_dir, 'takes', f'{rid}.flac')
    wav_path = os.path.join(scratch, f'{language}.wav')
    subprocess.run(
        ['afconvert', '-f', 'WAVE', '-d', 'LEI16@16000', '-c', '1', take, wav_path], check=True
    )
    with wave.open(wav_path) as w:
        assert (w.getnchannels(), w.getsampwidth(), w.getframerate()) == (1, 2, 16000)
        pcm = w.readframes(w.getnframes())
    return pcm, [s for s in segments if s[0] is not None and s[1] is not None]


def cut(source, start, end):
    pcm, segments = source
    text = ' '.join(t for a, b, t in segments if a >= start and b <= end)
    return pcm[start * BYTES_PER_MS:end * BYTES_PER_MS], text


def build(data_dir, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    with tempfile.TemporaryDirectory() as scratch:
        sources = {lang: load_source(data_dir, lang, scratch) for lang in SOURCES}
    gap = b'\0\0' * (GAP_MS * 16)
    refs, langs = [], []
    for name, turns in CLIPS.items():
        pieces = [cut(sources[lang], a, b) for lang, a, b in turns]
        with wave.open(os.path.join(out_dir, f'{name}.wav'), 'wb') as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(16000)
            w.writeframes(gap.join(pcm for pcm, _ in pieces))
        refs.append(f'{name}.wav\t' + ' '.join(text for _, text in pieces))
        at, parts = 0, []
        for (lang, a, b), (_, text) in zip(turns, pieces):
            parts.append(f'{lang}:{at}-{at + b - a}:{len(text.split())}')
            at += b - a + GAP_MS
        langs.append(f'{name}.wav\t' + ' '.join(parts))
    with open(os.path.join(out_dir, 'langs.tsv'), 'w') as f:
        f.write('\n'.join(langs) + '\n')
    with open(os.path.join(out_dir, 'refs.tsv'), 'w') as f:
        f.write('\n'.join(refs) + '\n')


def main(argv):
    if argv == ['--list']:
        print('\n'.join(CLIPS))
        return
    if len(argv) != 2:
        sys.exit(__doc__)
    data_dir, out_dir = argv
    if all(os.path.isfile(p) for p in outputs(out_dir)):
        print(f'turn-switch clips already built in {out_dir}')
        return
    build(data_dir, out_dir)
    print(f'built {len(CLIPS)} turn-switch clips in {out_dir}')


main(sys.argv[1:])
