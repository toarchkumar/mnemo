"""Reference embedder for `mnemo` --embed-cmd / MCP serve (Phase 3 PR 10b).

Protocol:
  * Read one JSON-encoded string per line from stdin.
  * Write one JSON array of floats per line to stdout.
  * Flush stdout after every reply, or mnemo will time out waiting.
  * Close on EOF of stdin (which mnemo sends when the handle drops).

The script runs persistently — one spawn per `Mnemo` handle, reused for
every subsequent remember/recall/cache call. Keep heavy imports (model
load) at module scope so the cost is paid once.

**Stdout is reserved for replies only.** Any `print(...)` without
`file=sys.stderr` — a startup banner, a "model loaded" line, a
progress bar, a warning — shifts every subsequent reply by one line
and mnemo's EmbedCmd will kill the child with a clear error.
Log to stderr instead: `print("...", file=sys.stderr, flush=True)`.
`sentence-transformers`/`transformers`/`torch` already log to stderr
by default, so this is only a hazard for your own debug prints.

Usage::

    # CLI, one-shot:
    mnemo remember notes.mnemo \\
        --text "the user prefers tea" \\
        --embed-cmd "python3 embedder_server.py" \\
        --embed-name "sentence-transformers/all-MiniLM-L6-v2"

    # MCP, persistent server:
    MNEMO_PASSPHRASE=... mnemo serve --mcp notes.mnemo \\
        --embed-cmd "python3 embedder_server.py" \\
        --embed-name "sentence-transformers/all-MiniLM-L6-v2"

Requires `sentence-transformers` + `torch`. Swap the model slug (and
pass the matching --embed-name) to use something else; just keep the
database's `dimensions` consistent with the embedder's output size.
"""

import json
import sys

from sentence_transformers import SentenceTransformer

MODEL = "sentence-transformers/all-MiniLM-L6-v2"  # 384-dim, normalized

model = SentenceTransformer(MODEL)

for line in sys.stdin:
    text = json.loads(line)
    vector = model.encode([text], normalize_embeddings=True)[0].tolist()
    print(json.dumps(vector), flush=True)
