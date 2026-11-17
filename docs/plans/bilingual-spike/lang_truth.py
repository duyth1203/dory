"""Wrong-language rate per chunk for the turn-switch set. Truth comes from the part times."""
import json, sys
parts = {
    'pure_tl0': [('tl', 24960)], 'pure_tl1': [('tl', 20880)], 'pure_tl2': [('tl', 21360)], 'pure_tl3': [('tl', 17220)],
    'pure_en0': [('en', 21440)], 'pure_en1': [('en', 19680)],
    'mix_tl3_en0': [('tl', 17220), ('en', 10000)], 'mix_en2_tl3': [('en', 10640), ('tl', 17220)],
    'mix_tl1_en3': [('tl', 20880), ('en', 8640)], 'mix_en1_tl3': [('en', 8560), ('tl', 17220)],
    'mix_en_tl3_en': [('en', 5040), ('tl', 17220), ('en', 3760)],
}
GAP = 400
wrong = total = mixed = 0
for line in open(sys.argv[1]):
    r = json.loads(line)
    spans, t = [], 0
    for lang, ms in parts[r['file'][:-4]]:
        spans.append((t, t + ms, lang)); t += ms + GAP
    for c in r['chunks']:
        a, b = c['start'] * 1000, c['end'] * 1000
        share = {}
        for s, e, lang in spans:
            share[lang] = share.get(lang, 0) + max(0, min(b, e) - max(a, s))
        truth = max(share, key=share.get)
        if len([v for v in share.values() if v > 300]) > 1:
            mixed += 1
        total += 1
        if c['lid'] != truth:
            wrong += 1
            print('wrong:', r['file'], round(c['start'], 1), round(c['end'], 1), 'heard', c['lid'], 'truth', truth, share, c['auto'][:80])
print(f'chunks {total}, wrong language {wrong} ({100*wrong/total:.1f}%), chunks spanning a switch {mixed}')
whole_wrong = sum(1 for l in open(sys.argv[1]) for r in [json.loads(l)] if len(parts[r['file'][:-4]]) > 1)
