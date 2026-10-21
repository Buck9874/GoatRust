"""Print decompiled functions from out/decomp/native by address or name.

usage: python -I tools/fn.py 01950040 [AActor_execMoveSmooth ...] [--raw]
Without --raw, Ghidra's rsqrtss normalisation boilerplate is dropped.
"""
import bisect
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
nd = os.path.join(root, 'out', 'decomp', 'native')
files = sorted(f for f in os.listdir(nd) if f.startswith('native_') and f.endswith('.c') and not re.match(r'native_(extra|pass2|retry)_', f))
starts = [int(f[7:15], 16) for f in files]
# second pass (tools/ghidra/DecompileMissing.java): vtable-only functions, any address order
extra = {}
for f in sorted(os.listdir(nd)):
    if re.match(r'native_(extra|pass2|retry)_', f) and f.endswith('.c'):
        with open(os.path.join(nd, f), encoding='utf-8') as fh:
            for line in fh:
                m = re.match(r'// ([0-9a-f]{8}) ', line)
                if m:
                    extra[int(m.group(1), 16)] = f
raw = '--raw' in sys.argv
names = {}
for tsv in ('functions.tsv', 'exec_names.tsv'):
    p = os.path.join(nd, tsv)
    if os.path.exists(p):
        with open(p, encoding='utf-8') as fh:
            next(fh)
            for line in fh:
                a, n = line.split('\t')[:2]
                names.setdefault(n, int(a, 16))
noise = re.compile(r'DAT_02b14c|uRam02b14|_DAT_02b14|^\s*}\s*$|^\s*$')
for arg in sys.argv[1:]:
    if arg.startswith('--'):
        continue
    addr = names.get(arg)
    if addr is None:
        addr = int(arg, 16)
    f = extra.get(addr) or files[max(0, bisect.bisect_right(starts, addr) - 1)]
    head = f'// {addr:08x} '
    out, on = [], False
    with open(os.path.join(nd, f), encoding='utf-8') as fh:
        for line in fh:
            if line.startswith('// ') and re.match(r'// [0-9a-f]{8} ', line):
                if on:
                    break
                on = line.startswith(head)
            if on and (raw or not noise.search(line) or line.strip() == '}'):
                out.append(line.rstrip('\n'))
    print('\n'.join(out) if out else f'{arg}: not found in {f}')
