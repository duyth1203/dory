import json, sqlite3, sys, time, urllib.request
sp = sys.argv[1]
db = sqlite3.connect(f'file:{sp}/lib/dory.sqlite?mode=ro', uri=True)
rows = db.execute("select start_ms, text from segments where recording_id='2026-11-16T041200-0000' order by idx").fetchall()
transcript = ''.join(f"[{a//60000:02}:{a//1000%60:02}] Speaker 1: {t}\n" for a, t in rows)

def sections(lang):
    return (f"Write the whole summary, headings included, in {lang}. Use Markdown with three sections in this order: "
            "key points, decisions, and action items. Keep each key point to one short line and merge points that repeat. "
            "List every decision and every action item, each once, with its owner and due date when the input names them. "
            "Write \"None\" under a section with nothing to report.")

def chat(system, user):
    body = json.dumps({"model": "gemma4:e4b", "stream": False, "options": {"temperature": 0},
                       "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}]}).encode()
    t = time.time()
    r = json.load(urllib.request.urlopen(urllib.request.Request("http://localhost:11434/api/chat", body, {"Content-Type": "application/json"}), timeout=600))
    wall = time.time() - t
    return r["message"]["content"], wall, r.get("prompt_eval_count"), r.get("prompt_eval_duration", 0) / 1e9, r.get("eval_count"), r.get("eval_duration", 0) / 1e9

def report(name, res):
    text, wall, pin, pdur, out, odur = res
    print(f"== {name}: wall {wall:.1f}s, prompt {pin} tok in {pdur:.1f}s ({pin/max(pdur,1e-9):.0f} tok/s), output {out} tok in {odur:.1f}s ({out/max(odur,1e-9):.0f} tok/s)")
    print(text.strip()[:900]); print()
    return text

chat("Reply OK.", "OK")  # warm the model so load time is not counted
tl = report("Tagalog summary from the transcript (today's path, language tl)",
            chat("You summarize meeting transcripts. " + sections("the language with BCP 47 tag tl") + " Use only what the transcript says.", transcript))
report("A. English summary written again from the transcript",
       chat("You summarize meeting transcripts. " + sections("the language with BCP 47 tag en") + " Use only what the transcript says.", transcript))
report("B. The Tagalog summary translated into English",
       chat("You translate a meeting summary written in Markdown into the language with BCP 47 tag en. Keep the Markdown structure, "
            "every heading, bullet, name, number, and date. Translate headings too. Reply with the translated Markdown only.", tl))
