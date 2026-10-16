import re
import sys
from pathlib import Path


def keys(path):
    return {m[1] for m in re.finditer(r"^([A-Za-z_]\w*)=", path.read_text(), re.M)}


env = Path(".env")
if not env.exists():
    sys.exit(".env missing, copy .env.example to .env")
missing = sorted(keys(Path(".env.example")) - keys(env))
if missing:
    sys.exit("keys missing from .env:\n" + "\n".join(f"  {k}" for k in missing))
print(".env ok")
