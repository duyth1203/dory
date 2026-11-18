"""Scores bilingual-spike output. Throwaway.

usage: python3 -I score.py <results.jsonl> <refs.tsv>
"""
import json, re, sys
from collections import Counter

TL = set("""
ang ng mga sa na ay at si ni kay ko mo niya namin natin nila ako ikaw siya kami tayo kayo sila
ito iyan iyon dito diyan doon hindi oo po opo din rin lang lamang pa naman nga kasi dahil kung
kapag para pero ngunit subalit bagaman kaya upang nang may mayroon wala walang isang isa dalawa
ano sino saan kailan bakit paano yung yun yan ba daw raw talaga sobrang mas pinaka kanya kaniyang
kanilang aming ating inyong kanila amin atin inyo akin iyo kanino ating nasa noon ngayon bago
mula hanggang tungkol laban gayon gayundin dapat kailangan maaari puwede pwede siguro baka
""".split())

DICT = None


def english(word):
    global DICT
    if DICT is None:
        DICT = {w.strip().lower() for w in open('/usr/share/dict/words')}
    if word in TL:
        return False
    if word in DICT:
        return True
    for suffix in ('s', 'es', 'ed', 'ing', 'ly', "'s"):
        if word.endswith(suffix) and word[: -len(suffix)] in DICT:
            return True
    return False


def words(text):
    text = text.lower()
    text = re.sub(r"[^\w']+", ' ', text)
    return [w for w in text.split() if w]


def edits(r, h):
    row = list(range(len(h) + 1))
    for i, a in enumerate(r):
        diag, row[0] = row[0], i + 1
        for j, b in enumerate(h):
            sub = diag + (a != b)
            diag = row[j + 1]
            row[j + 1] = min(sub, row[j] + 1, row[j + 1] + 1)
    return row[len(h)]


def recall(ref, hyp, pick):
    want = Counter(w for w in ref if pick(w))
    have = Counter(hyp)
    total = sum(want.values())
    hit = sum(min(n, have[w]) for w, n in want.items())
    return hit, total


def main():
    rows = [json.loads(l) for l in open(sys.argv[1])]
    refs = dict(l.rstrip('\n').split('\t', 1) for l in open(sys.argv[2]))

    def chunked(row, mode):
        if mode == 'routed':
            return ' '.join(c['auto'] if c['lid'] == 'en' else c['tl'] for c in row['chunks'])
        return ' '.join(c[mode] for c in row['chunks'])

    variants = {
        'whole window, auto': lambda r: r['auto'],
        'whole window, pinned tl': lambda r: r['tl'],
        'whole window, pinned en': lambda r: r['en'],
        '6 s chunks, auto': lambda r: chunked(r, 'auto'),
        '6 s chunks, pinned tl': lambda r: chunked(r, 'tl'),
        '6 s chunks, tl unless auto hears en': lambda r: chunked(r, 'routed'),
        'best of auto and pinned tl per window (oracle)': None,
        'best of pinned tl and pinned en per window (oracle)': None,
    }
    secs_of = {
        'whole window, auto': lambda r: r['auto_s'],
        'whole window, pinned tl': lambda r: r['tl_s'],
        'whole window, pinned en': lambda r: r['en_s'],
        '6 s chunks, auto': lambda r: sum(c['auto_s'] for c in r['chunks']),
        '6 s chunks, pinned tl': lambda r: sum(c['tl_s'] for c in r['chunks']),
        '6 s chunks, tl unless auto hears en': lambda r: sum(c['auto_s'] + (0 if c['lid'] == 'en' else c['tl_s']) for c in r['chunks']),
        'best of auto and pinned tl per window (oracle)': lambda r: r['auto_s'] + r['tl_s'],
        'best of pinned tl and pinned en per window (oracle)': lambda r: r['tl_s'] + r['en_s'],
    }
    audio = sum(r['secs'] for r in rows)
    verbose = '-v' in sys.argv
    print(f"{len(rows)} clips, {audio:.0f} s of audio")
    print('variant | WER | Tagalog words kept | English words kept | clips turned into English | RTFx')
    print('--- | --- | --- | --- | --- | ---')
    def best(keys):
        def pick(r):
            ref = words(refs[r['file']])
            return min((r[k] for k in keys), key=lambda h: edits(ref, words(h)))
        return pick
    variants['best of auto and pinned tl per window (oracle)'] = best(['auto', 'tl'])
    variants['best of pinned tl and pinned en per window (oracle)'] = best(['tl', 'en'])
    for name, hyp_of in variants.items():
        err = tot = th = tt = eh = et = flipped = 0
        for r in rows:
            ref = words(refs[r['file']])
            hyp = words(hyp_of(r))
            err += edits(ref, hyp); tot += len(ref)
            a, b = recall(ref, hyp, lambda w: not english(w)); th += a; tt += b
            a, b = recall(ref, hyp, english); eh += a; et += b
            ref_tl = sum(not english(w) for w in ref) / max(1, len(ref))
            hyp_tl = sum(not english(w) for w in hyp) / max(1, len(hyp))
            if ref_tl >= 0.2 and hyp_tl < ref_tl / 2:
                flipped += 1
                if verbose:
                    print(f"  flipped {name} {r['file']}: {hyp_of(r)[:160]}")
        busy = sum(secs_of[name](r) for r in rows)
        print(f"{name} | {100*err/tot:.1f}% | {100*th/max(1,tt):.0f}% | {100*eh/max(1,et):.0f}% | {flipped}/{len(rows)} | {audio/busy:.1f}")
    lids = Counter(r['lid'] for r in rows)
    chunk_lids = Counter(c['lid'] for r in rows for c in r['chunks'])
    nchunks = sum(len(r['chunks']) for r in rows)
    print(f"\nwhole-clip language guesses: {dict(lids)}")
    print(f"chunk language guesses ({nchunks} chunks): {dict(chunk_lids)}")
    if verbose:
        for r in rows:
            print(r['file'], r['lid'], [c['lid'] for c in r['chunks']])


if '--cover' not in sys.argv:
    main()


def truncation(path, refs_path):
    rows = [json.loads(l) for l in open(path)]
    refs = dict(l.rstrip('\n').split('\t', 1) for l in open(refs_path))
    print('\nfile | ref words | whole auto words | kept | words per second | chunks auto words')
    print('--- | --- | --- | --- | --- | ---')
    for r in rows:
        ref = words(refs[r['file']])
        whole = words(r['auto'])
        chunk = words(' '.join(c['auto'] for c in r['chunks']))
        print(f"{r['file']} | {len(ref)} | {len(whole)} | {len(whole)/max(1,len(ref)):.2f} | {len(whole)/r['secs']:.2f} | {len(chunk)}")


if '--truncation' in sys.argv:
    truncation(sys.argv[1], sys.argv[2])


def cover(path, refs_path):
    rows = [json.loads(l) for l in open(path)]
    refs = dict(l.rstrip('\n').split('\t', 1) for l in open(refs_path))
    for key in ('first', 'cover'):
        err = tot = th = tt = eh = et = flipped = 0
        for r in rows:
            ref = words(refs[r['file']]); hyp = words(r[key])
            err += edits(ref, hyp); tot += len(ref)
            a, b = recall(ref, hyp, lambda w: not english(w)); th += a; tt += b
            a, b = recall(ref, hyp, english); eh += a; et += b
            ref_tl = sum(not english(w) for w in ref) / max(1, len(ref))
            hyp_tl = sum(not english(w) for w in hyp) / max(1, len(hyp))
            if ref_tl >= 0.2 and hyp_tl < ref_tl / 2:
                flipped += 1
        print(f"{key} | {100*err/tot:.1f}% | {100*th/max(1,tt):.0f}% | {100*eh/max(1,et):.0f}% | {flipped}/{len(rows)}")
    audio = sum(r['secs'] for r in rows)
    busy = sum(r['cover_s'] for r in rows)
    gaps = sum(len(r['gaps']) for r in rows)
    print(f"RTFx with re-decodes {audio/busy:.1f}; windows with a gap {sum(1 for r in rows if r['gaps'])}/{len(rows)}; gaps {gaps}")
    print('gap languages', Counter(g['lang'] for r in rows for g in r['gaps']))
    for r in rows:
        if r['gaps']:
            print(' ', r['file'], r['lid'], [(round(g['start'], 1), round(g['end'], 1), g['lang']) for g in r['gaps']], '|', r['cover'][-120:])


if '--cover' in sys.argv:
    cover(sys.argv[1], sys.argv[2])
