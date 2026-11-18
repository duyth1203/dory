import sqlite3, sys, wave, os
sp = sys.argv[1]
db = sqlite3.connect(f'file:{sp}/lib/dory.sqlite?mode=ro', uri=True)
seg = {}
for rid, idx, a, b, text in db.execute("select recording_id, idx, start_ms, end_ms, text from segments"):
    seg.setdefault('tl' if rid.endswith('041200-0000') else 'en', []).append((a, b, text))
def pcm(lang):
    w = wave.open(f'{sp}/data/{lang}.wav'); return w.readframes(w.getnframes())
audio = {k: pcm(k) for k in ('tl', 'en')}
def cut(lang, a, b):
    parts = [s for s in seg[lang] if s[0] >= a and s[1] <= b]
    return audio[lang][a * 32: b * 32], ' '.join(t for _, _, t in parts), lang
gap = b'\0\0' * 6400
clips = {
    'pure_tl0': [('tl', 0, 24960)], 'pure_tl1': [('tl', 26850, 47730)],
    'pure_tl2': [('tl', 50070, 71430)], 'pure_tl3': [('tl', 72000, 89220)],
    'pure_en0': [('en', 0, 21440)], 'pure_en1': [('en', 21440, 41120)],
    'mix_tl3_en0': [('tl', 72000, 89220), ('en', 0, 10000)],
    'mix_en2_tl3': [('en', 21440, 32080), ('tl', 72000, 89220)],
    'mix_tl1_en3': [('tl', 26850, 47730), ('en', 32480, 41120)],
    'mix_en1_tl3': [('en', 5040, 13600), ('tl', 72000, 89220)],
    'mix_en_tl3_en': [('en', 0, 5040), ('tl', 72000, 89220), ('en', 37360, 41120)],
}
out = f'{sp}/data/turns'
with open(f'{out}/refs.tsv', 'w') as refs, open(f'{out}/langs.tsv', 'w') as langs:
    for name, parts in clips.items():
        pieces = [cut(*p) for p in parts]
        w = wave.open(f'{out}/{name}.wav', 'wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000)
        w.writeframes(gap.join(p[0] for p in pieces)); w.close()
        refs.write(f'{name}.wav\t' + ' '.join(p[1] for p in pieces) + '\n')
        langs.write(f'{name}.wav\t' + ' '.join(f'{p[2]}:{len(p[1].split())}' for p in pieces) + '\n')
