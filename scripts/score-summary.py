"""Action-item recall of a summary of the test library's "Two hour planning" meeting.

usage: python3 -I scripts/score-summary.py <summary.md>...

The meeting assigns 40 action items, each "<owner> will finish the <item> by <day>", four times
over. A summary keeps an item when its Action Items section names it, and keeps its owner when
the same line also starts the owner's name. Item patterns accept the spellings the speech engines
wrote, such as "ordered log" for "audit log". Copied from the platform AI spike's score.py
(docs/plans/platform-ai.md, Appendix A).

The spike's saved summaries. Gemma kept 36 of 40, Apple Intelligence 4.

>>> score(GEMMA_SPIKE)
(36, 29, 37)
>>> missing(GEMMA_SPIKE)
['beta survey', 'sales deck', 'latency alerts', 'feature flags']
>>> score(APPLE_SPIKE)
(4, 4, 4)
>>> score("## Key Points\\n* Maria will finish the billing dashboard.\\n")
(0, 0, 0)
"""
import re
import sys

ITEMS = [
    ("Maria", "billing dashboard", r"billing"),
    ("Daniel", "onboarding emails", r"onboarding e"),
    ("Priya", "Android crash", r"android"),
    ("Kenji", "search index", r"search index"),
    ("Sofia", "pricing page", r"pricing"),
    ("Omar", "data export", r"data export"),
    ("Lina", "audit log", r"audit|ordered? log"),
    ("Thomas", "help center", r"help cent"),
    ("Aisha", "payment retry", r"payment retr"),
    ("Marco", "sign in flow", r"sign.?in"),
    ("Maria", "release checklist", r"release checklist"),
    ("Daniel", "usage report", r"usage report"),
    ("Priya", "partner API", r"partner api"),
    ("Kenji", "cache layer", r"cache|cash"),
    ("Sofia", "support macros", r"support macro"),
    ("Omar", "trial limits", r"trial limit"),
    ("Lina", "invoice template", r"invoice template"),
    ("Thomas", "status page", r"status page"),
    ("Aisha", "referral program", r"referral"),
    ("Marco", "mobile widgets", r"widget"),
    ("Maria", "Q3 budget", r"budget"),
    ("Daniel", "hiring plan", r"hiring"),
    ("Priya", "design tokens", r"design token"),
    ("Kenji", "beta survey", r"beta survey"),
    ("Sofia", "sales deck", r"sales deck"),
    ("Omar", "backup policy", r"backup"),
    ("Lina", "latency alerts", r"latency"),
    ("Thomas", "feature flags", r"feature flag"),
    ("Aisha", "accessibility review", r"accessibility"),
    ("Marco", "translation files", r"translation file"),
    ("Maria", "roadmap draft", r"roadmap"),
    ("Daniel", "security patch", r"security patch"),
    ("Priya", "analytics events", r"analytics"),
    ("Kenji", "team offsite", r"off.?site|of site"),
    ("Sofia", "vendor contract", r"vendor"),
    ("Omar", "load test", r"load test"),
    ("Lina", "docs site", r"docs? site|dock site"),
    ("Thomas", "privacy notice", r"privacy notice"),
    ("Aisha", "dark mode", r"dark mode"),
    ("Marco", "legacy importer", r"legacy import"),
]


def section(md, name):
    m = re.search(r"#+\s*" + name + r".*?\n(.*?)(?=\n#+\s|\Z)", md, re.I | re.S)
    return m.group(1) if m else ""


def score(md):
    """(items kept, items kept with their owner, bullets) in the Action Items section."""
    acts = section(md, "action items").lower()
    hit = [t for o, t, rx in ITEMS if re.search(rx, acts)]
    owner = [
        t
        for o, t, rx in ITEMS
        if any(re.search(rx, l) and o.lower()[:3] in l for l in acts.splitlines())
    ]
    bullets = acts.count("\n*") + acts.count("\n-")
    return len(hit), len(owner), bullets


def missing(md):
    acts = section(md, "action items").lower()
    return [t for o, t, rx in ITEMS if not re.search(rx, acts)]


GEMMA_SPIKE = """# Action Items

* Maria to finish the billing dashboard by Monday.
* Maria to finish the release checklist by Monday.
* Maria to finish the error budget by Monday.
* Maria to finish the roadmap draft by Monday.
* Omar to finish the data export by Monday.
* Omar to finish the trial limits by Monday.
* Omar to finish the load test by Monday.
* Omar to finish the backup policy by Monday.
* Daniel to finish the onboarding emails by Thursday.
* Daniel to finish the usage report by Thursday.
* Daniel to finish the hiring plan by Thursday.
* Daniel to finish the security patch by Thursday.
* Lima to finish the audit logged by Thursday.
* Lima to finish the invoice template by Thursday.
* Lima to finish the dock site by Thursday.
* Thomas to finish the privacy notice by Tuesday.
* Thomas to finish the status page by Tuesday.
* Thomas to finish the help center by Tuesday.
* Prio to finish the Android crash by Tuesday.
* Prio to finish the partner API by Tuesday.
* Prio to finish the design tokens by Tuesday.
* Prio to finish the analytics events by Tuesday.
* Kenji to finish the search index by Friday.
* Kenji to finish the cash layer by Friday.
* Kenji to finish the team off site by Friday.
* Sofia to finish the pricing page by Wednesday.
* Sofia to finish the support macros by Wednesday.
* Sofia to finish the vendor contract by Wednesday.
* Marco to finish the sign in flow by Wednesday.
* Marco to finish the mobile widgets by Wednesday.
* Marco to finish the legacy importer by Wednesday.
* Marco to finish the translation files by Wednesday.
* Ayesha to finish the payment retry by Friday.
* Ayesha to finish the referral program by Friday.
* Ayesha to finish the dark mode by Friday.
* Ayesha to finish the accessibility review by Friday.
* Lena to finish the dock site by Thursday."""

APPLE_SPIKE = """## Decisions

* Maria will finish the billing dashboard by Monday.

## Action Items

* Maria will finish the billing dashboard by Monday.
* Daniel will finish the onboarding emails by Thursday.
* Maria will finish the roadmap draft by Monday.
* Daniel will finish the security patch by Thursday."""


def main(paths):
    for f in paths:
        md = open(f).read()
        hit, owner, bullets = score(md)
        print(f"{f}: action_items_found={hit}/40 with_owner={owner}/40 bullets={bullets} chars={len(md)}")
        print("   missing:", ", ".join(missing(md)))


if __name__ == "__main__":
    main(sys.argv[1:])
