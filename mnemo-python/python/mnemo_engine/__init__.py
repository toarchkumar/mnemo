"""The `mnemo_engine` public Python API.

Thin pure-Python shim that re-exports every public name from the
compiled extension (the `.so` / `.pyd` file maturin places inside
this package). In practice you can treat this module exactly as the
compiled extension — the only reason to have a Python file at all is
maturin's mixed-layout contract, which requires an `__init__.py` to
live alongside the native module.
"""

from __future__ import annotations

from .mnemo_engine import *  # noqa: F401,F403 — surface every public name
from .mnemo_engine import __version__  # noqa: F401 — explicit re-export

# Everything public comes from the compiled extension. Mirror its
# `__all__` so `from mnemo_engine import *` works identically here.
try:
    from .mnemo_engine import __all__  # type: ignore[attr-defined]  # noqa: F401
except ImportError:  # pragma: no cover
    # The compiled extension doesn't define __all__ yet (NAME-1 did
    # not add one — we rely on the "no leading underscore" default).
    pass
