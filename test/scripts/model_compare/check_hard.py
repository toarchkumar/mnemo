#!/usr/bin/env python3
"""Check questions_hard.json against step 5 part 3's group rules (12.3).

It runs no embedding model. It reads the brain read-only (the passphrase
from test/.env, through project_memory.py), PLAN.md, and MiniLM's
tokenizer.json (for the long group), and rejects any question that breaks
its group's rule:

- every snippet (expected, stale, answer) appears in PLAN.md and in exactly
  one memory;
- ident: `identifier` looks like one (a function, constant, flag, feature,
  path or crate@version) and appears verbatim in the question and in the
  expected fact;
- paraphrase: the question shares no content word with the expected fact,
  after lowercasing, removing STOPWORDS and stemming plurals;
- long: the expected fact is over 256 MiniLM tokens and `answer_snippet`
  starts after token 256; the group has one question per such fact, up
  to 5;
- current: `stale` names a different memory than `expected`;
- pair: two expected snippets, two different memories;
- the group sizes: ident 10, paraphrase 10, current 5, pair 5.

Usage: check_hard.py <path to MiniLM tokenizer.json>
"""
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
sys.path.insert(0, str(HERE.parent))
import project_memory as pm  # noqa: E402

STOPWORDS = set("""
a an the and or but if then so of to in on at by for with from into onto over under
is are was were be been being am do does did done doing have has had having
i me my we our us you your he she it its they them their this that these those
what which who whom whose when where why how whether
can could may might must shall should will would not no nor only own same than too very
just also still yet now ever all any each every both few more most other some such
about after again against before below between during off out up down further once
there here as get gets got make makes made use used using one two s t
""".split())

IDENT = re.compile(r"(::|_|@|=|--|\(|\.[A-Za-z]|[a-z][A-Z])")


def words(text):
    out = set()
    for w in re.findall(r"[a-z0-9]+", text.lower()):
        if len(w) < 2 or w in STOPWORDS:
            continue
        if w.endswith("ies") and len(w) > 4:
            w = w[:-3] + "y"
        elif re.search(r"(ss|x|ch|sh)es$", w):
            w = w[:-2]
        elif w.endswith("s") and not w.endswith("ss") and len(w) > 3:
            w = w[:-1]
        out.add(w)
    return out


def main():
    from tokenizers import Tokenizer

    tok = Tokenizer.from_file(sys.argv[1])
    tok.no_truncation()
    tok.no_padding()
    plan = re.sub(r"\s+", " ", (REPO / "PLAN.md").read_text())
    mnemo = pm.require_mnemo()
    db = mnemo.open(str(pm.MNEMO_PATH), pm.passphrase(), read_only=True)
    mems = [re.sub(r"\s+", " ", m["content"]) for m in db.memories()]
    db.close()
    data = json.loads((HERE / "questions_hard.json").read_text())
    qs = data["questions"]
    bad = []

    def one(qid, snippet):
        if snippet not in plan:
            bad.append(f"{qid}: snippet not in PLAN.md: {snippet!r}")
        hits = [i for i, c in enumerate(mems) if snippet in c]
        if len(hits) != 1:
            bad.append(f"{qid}: snippet matches {len(hits)} memories: {snippet!r}")
            return None
        return hits[0]

    long_facts = [i for i, c in enumerate(mems) if len(tok.encode(c).ids) > 256]
    counts = {}
    for q in qs:
        qid, g, text = q["id"], q["group"], q["question"]
        counts[g] = counts.get(g, 0) + 1
        ids = [one(qid, s) for s in q["expected"]]
        if None in ids:
            continue
        if g == "ident":
            ident = q.get("identifier", "")
            if not IDENT.search(ident):
                bad.append(f"{qid}: {ident!r} doesn't look like an identifier")
            if ident not in text:
                bad.append(f"{qid}: identifier not in the question")
            if not all(ident in mems[i] for i in ids):
                bad.append(f"{qid}: identifier not verbatim in the expected fact")
        elif g == "paraphrase":
            shared = words(text) & set().union(*(words(mems[i]) for i in ids))
            if shared:
                bad.append(f"{qid}: shares content words {sorted(shared)}")
        elif g == "long":
            i = ids[0]
            if i not in long_facts:
                bad.append(f"{qid}: the fact isn't over 256 tokens")
            ans = q["answer_snippet"]
            if one(qid, ans) != i:
                bad.append(f"{qid}: answer_snippet isn't in the expected fact")
            else:
                enc = tok.encode(mems[i])
                start = mems[i].index(ans)
                after = [k for k, (a, _) in enumerate(enc.offsets) if a >= start]
                if not after or after[0] <= 256:
                    bad.append(f"{qid}: the answer starts at token {after[0] if after else '?'}, not after 256")
        elif g == "current":
            stale = [one(qid, s) for s in q["stale"]]
            if None in stale or set(stale) & set(ids):
                bad.append(f"{qid}: the stale fact must be another memory")
        elif g == "pair":
            if len(set(ids)) != 2:
                bad.append(f"{qid}: a pair needs two different memories")
        else:
            bad.append(f"{qid}: unknown group {g}")
    want = {"ident": 10, "paraphrase": 10, "current": 5, "pair": 5,
            "long": min(5, len(long_facts))}
    for g, n in want.items():
        if counts.get(g, 0) != n:
            bad.append(f"group {g}: {counts.get(g, 0)} questions, the rule wants {n}")
    print(f"{len(qs)} questions; {len(long_facts)} fact(s) over 256 tokens; groups {counts}")
    for b in bad:
        print("REJECT", b)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
