"""Deprecated-import shim for the mnemo-engine wheel.

``import mnemo`` kept working for the whole 0.6.x series as the import
name of the compiled extension; starting in 0.7.0 the extension's
import name is ``mnemo_engine`` (PR NAME-1 — see the ``Breaking:
import name`` section in CHANGELOG.md for the rationale). This shim
re-exports every public name from :mod:`mnemo_engine` so existing code
keeps working through the 0.7.x series; it is removed in 0.8.0.

Users should migrate to one of these two patterns:

* ``import mnemo_engine`` — the direct new name.
* ``import mnemo_engine as mnemo`` — a drop-in that keeps call sites
  unchanged without triggering the deprecation warning.
"""

from __future__ import annotations

import warnings as _warnings

import mnemo_engine as _m

# Emit the deprecation once per import, not once per name lookup.
_warnings.warn(
    "`import mnemo` is deprecated and will stop working in mnemo-engine "
    "0.8.0 — the extension's import name is now `mnemo_engine`. "
    "Migrate with `import mnemo_engine` or keep call sites unchanged "
    "via `import mnemo_engine as mnemo`. The alias stays through 0.7.x "
    "(see the CHANGELOG `Breaking: import name` section for the full "
    "rationale and both migration paths).",
    DeprecationWarning,
    stacklevel=2,
)

# Re-export every public name the extension module exposes. We walk
# __all__ when the extension defines one, and otherwise re-export
# everything that doesn't start with an underscore — which covers the
# classes (Mnemo, Session, Turn) and functions (open, compact_file,
# build_info) listed in the NAME-1 dispatch. __version__ is
# re-exported explicitly because callers check it.
__version__ = _m.__version__

_public = getattr(_m, "__all__", None) or [
    _name for _name in dir(_m) if not _name.startswith("_")
]
for _name in _public:
    globals()[_name] = getattr(_m, _name)

# Expose the shim-synthesized public surface so `from mnemo import *`
# works without pulling private helpers.
__all__ = list(_public) + ["__version__"]

# Drop the loop variable so it doesn't leak into the module namespace.
del _name
