"""How many action-item sentences of the "Two hour planning" meeting a transcript got exactly right.

usage: python3 -I scripts/score-items.py <transcript.txt>...

The transcript has one segment's text per line, as `dory-cli show` prints it. Of the lines that
say "will finish", it counts those that name an owner and item together, as in "Maria will finish
the billing dashboard". The owners and items are `scripts/score-summary.py`'s, so the two
scripts score the same 40 items. Owner spellings such as Priya and Aisha are guesses, so the
score compares engines rather than grading one. Copied from the platform AI spike's score.py
(docs/plans/platform-ai.md, Appendix A), where Apple Speech got 105 of 160 and Parakeet 87 of 158.

Lines from the spike's saved transcripts, Apple Speech first, then Parakeet.

>>> score(APPLE_SPIKE)
(12, 9)
>>> score(PARAKEET_SPIKE)
(12, 9)
"""
import importlib.util
import re
import sys
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "score_summary", Path(__file__).with_name("score-summary.py")
)
_summary = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_summary)
ITEMS = _summary.ITEMS


def score(lines):
    """(item sentences, sentences with the exact owner and item)."""
    said = [l.strip() for l in lines if "will finish" in l.lower()]
    exact = sum(
        1
        for l in said
        if any(re.search(rf"\b{o}\b will finish the {t}\b", l, re.I) for o, t, rx in ITEMS)
    )
    return len(said), exact


APPLE_SPIKE = """ Maria will finish the billing dashboard by Monday.
 Daniel will finish the onboarding emails by Thursday.
 Prio will finish the Android crash by Tuesday.
 Kenji will finish the search index by Friday.
 Sofia will finish the pricing page by Wednesday.
 Omar will finish the data export by Monday.
 Lima will finish the ordered logged by Thursday.
 Thomas will finish the help center by Tuesday.
 Ayesha will finish the payment retried by Friday.
 Marco will finish the sign in flow by Wednesday.
 Maria will finish the release checklist by Monday.
 Daniel will finish the usage report by Thursday.""".splitlines()

PARAKEET_SPIKE = """Maria will finish the billing dashboard by Monday.
Daniel will finish the onboarding emails by Thursday.
Krea will finish the Android crash by Tuesday.
Kenji will finish the search index by Friday.
Sophia will finish the pricing page by Wednesday.
Omar will finish the data export by Monday.
Lena will finish the audit log by Thursday.
Thomas will finish the help center by Tuesday.
Aisha will finish the payment retry by Friday.
Marco will finish the sign in flow by Wednesday.
Maria will finish the release checklist by Monday.
Daniel will finish the usage report by Thursday.""".splitlines()


def main(paths):
    for f in paths:
        said, exact = score(open(f))
        print(f"{f}: item_sentences={said} exact_owner_and_item={exact} ({100 * exact / max(said, 1):.0f}%)")


if __name__ == "__main__":
    main(sys.argv[1:])
